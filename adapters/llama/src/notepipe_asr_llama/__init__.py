"""Serve the notepipe transcription contract using a managed llama-server."""
import argparse
import base64
from collections import deque
from email import policy
from email.parser import BytesParser
import http.client
import http.server
import json
import signal
import socket
import subprocess
import sys
import threading
import urllib.error
import urllib.request

MAX_UPLOAD = 64 * 1024 * 1024


def multipart(content_type, body):
    message = BytesParser(policy=policy.default).parsebytes(
        f"Content-Type: {content_type}\r\nMIME-Version: 1.0\r\n\r\n".encode() + body
    )
    if message.get_content_type() != "multipart/form-data" or not message.is_multipart():
        raise ValueError("Expected multipart/form-data with a boundary")
    fields = {}
    for part in message.iter_parts():
        name = part.get_param("name", header="content-disposition")
        if part.get_content_disposition() != "form-data" or name not in {"file", "model", "language"}:
            raise ValueError("Expected file, model or language multipart fields")
        if name in fields or part.defects:
            raise ValueError("Duplicate or malformed multipart field")
        value = part.get_payload(decode=True)
        if value is None:
            raise ValueError("Invalid multipart payload")
        fields[name] = value if name == "file" else value.decode("utf-8")
    if message.defects or not fields.get("file"):
        raise ValueError("A WAV file is required")
    return fields


def sanitize_qwen_text(text):
    text = text.strip()
    if "</asr_text>" in text:
        text = text.split("</asr_text>", 1)[0].strip()
    if "<asr_text>" in text:
        text = text.rsplit("<asr_text>", 1)[1].strip()
    return text


def transcription_request(fields):
    prompt = "Transcribe the audio."
    if fields.get("language"):
        prompt += f" The speech is in {fields['language']}."
    body = {
        "temperature": 0,
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": prompt},
            {"type": "input_audio", "input_audio": {
                "data": base64.b64encode(fields["file"]).decode("ascii"), "format": "wav"
            }},
        ]}],
    }
    if fields.get("model"):
        body["model"] = fields["model"]
    return json.dumps(body).encode("utf-8")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        return None


class Handler(http.server.BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(120)

    def send_error(self, code, message=None, explain=None):
        self.error(code, "Unsupported request")

    def reply(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def error(self, status, message):
        self.reply(status, {"error": {"message": message}})

    def do_GET(self):
        if self.path != "/health":
            self.error(404, "Unknown endpoint")
            return
        try:
            with urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect).open(self.server.base_url + "/health", timeout=2) as response:
                ready = response.status == 200
        except (OSError, http.client.HTTPException):
            ready = False
        if ready:
            self.reply(200, {"status": "ok"})
        else:
            self.error(503, "llama-server is not ready")

    def do_POST(self):
        if self.path != "/v1/audio/transcriptions":
            self.error(404, "Unknown endpoint")
            return
        try:
            length = int(self.headers.get("Content-Length", "-1"))
            if length < 0:
                self.error(411, "Content-Length is required")
                return
            if length > MAX_UPLOAD:
                self.error(413, "Audio upload exceeds 64 MiB")
                return
            body = self.rfile.read(length)
            if len(body) != length:
                raise ValueError("Truncated upload")
            fields = multipart(self.headers.get("Content-Type", ""), body)
        except (ValueError, OSError):
            self.error(400, "Invalid multipart upload")
            return

        try:
            request = urllib.request.Request(
                self.server.base_url + "/v1/chat/completions", data=transcription_request(fields),
                headers={"Content-Type": "application/json"}, method="POST",
            )
            with urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect).open(request, timeout=120) as response:
                raw = response.read(MAX_UPLOAD + 1)
            if len(raw) > MAX_UPLOAD:
                raise ValueError("Upstream response exceeds 64 MiB")
            value = json.loads(raw)
            if not isinstance(value, dict) or value.get("error") is not None:
                raise ValueError("Invalid upstream response")
            text = value["choices"][0]["message"]["content"]
            if not isinstance(text, str):
                raise ValueError("Invalid upstream transcript")
            text.encode("utf-8")
        except urllib.error.HTTPError as error:
            with error:
                phrase = self.responses.get(error.code, ("Request failed", ""))[0]
                self.error(error.code, f"Upstream HTTP {error.code}: {phrase}")
            return
        except (OSError, ValueError, KeyError, IndexError, TypeError, http.client.HTTPException):
            self.error(502, "Invalid response from llama-server")
            return
        self.reply(200, {"text": sanitize_qwen_text(text)})


def main():
    parser = argparse.ArgumentParser(description="llama.cpp transcription HTTP adapter", allow_abbrev=False, add_help=False)
    parser.add_argument("--help", action="help")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--llama-server", default="llama-server")
    args, passthrough = parser.parse_known_args()
    child = None
    stderr_thread = None
    stderr_tail = deque(maxlen=20)
    stopping = threading.Event()
    previous_signals = {}
    failed = False
    try:
        with http.server.ThreadingHTTPServer((args.host, args.port), Handler) as server:
            server.timeout = 0.2
            with socket.socket() as port_socket:
                port_socket.bind(("127.0.0.1", 0))
                port = port_socket.getsockname()[1]
            server.base_url = f"http://127.0.0.1:{port}"
            for sig in (signal.SIGINT, signal.SIGTERM):
                previous_signals[sig] = signal.signal(sig, lambda *_: stopping.set())
            child = subprocess.Popen(
                [args.llama_server, *passthrough, "--host", "127.0.0.1", "--port", str(port)],
                stdin=subprocess.DEVNULL, stdout=sys.stderr, stderr=subprocess.PIPE, text=True, errors="replace",
            )

            def collect_stderr():
                for line in iter(lambda: child.stderr.readline(4096), ""):
                    stderr_tail.append(line.rstrip())

            stderr_thread = threading.Thread(target=collect_stderr, daemon=True)
            stderr_thread.start()
            while not stopping.is_set():
                if child.poll() is not None:
                    failed = True
                    break
                server.handle_request()
    except (OSError, ValueError) as error:
        print(f"Could not run the llama.cpp transcription adapter: {error}", file=sys.stderr)
        failed = True
    finally:
        if child is not None:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            if stderr_thread is not None:
                stderr_thread.join()
            child.stderr.close()
        for sig, previous in previous_signals.items():
            signal.signal(sig, previous)
    if failed:
        if child is not None:
            print(f"llama-server exited with status {child.returncode}", file=sys.stderr)
            print("\n".join(stderr_tail), file=sys.stderr)
        raise SystemExit(1)
