"""Forward the notepipe transcription contract to xAI's speech API."""
import argparse
from email import policy
from email.parser import BytesParser
import http.client
import http.server
import json
import urllib.error
import urllib.request
import uuid

MAX_UPLOAD = 64 * 1024 * 1024
DEFAULT_BASE_URL = "https://api.x.ai/v1"


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


def transcription_request(fields):
    boundary = f"notepipe-{uuid.uuid4().hex}"
    parts = []
    for name in ("model", "language"):
        if fields.get(name):
            parts.append(
                f'--{boundary}\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n{fields[name]}\r\n'.encode()
            )
    parts.append(
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="audio.wav"\r\nContent-Type: audio/wav\r\n\r\n'.encode()
        + fields["file"] + f"\r\n--{boundary}--\r\n".encode()
    )
    return f"multipart/form-data; boundary={boundary}", b"".join(parts)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, newurl):
        # Credentials belong only to the configured upstream.
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
        if self.path == "/health":
            self.reply(200, {"status": "ok"})
        else:
            self.error(404, "Unknown endpoint")

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
            content_type, body = transcription_request(fields)
            headers = {"Content-Type": content_type}
            authorization = self.headers.get("Authorization")
            if authorization is not None:
                headers["Authorization"] = authorization
            request = urllib.request.Request(
                self.server.base_url.rstrip("/") + "/stt", data=body, headers=headers, method="POST"
            )
            with urllib.request.build_opener(NoRedirect).open(request, timeout=120) as response:
                raw = response.read(MAX_UPLOAD + 1)
            if len(raw) > MAX_UPLOAD:
                raise ValueError("Upstream response exceeds 64 MiB")
            value = json.loads(raw)
            if not isinstance(value, dict) or value.get("error") is not None or not isinstance(value.get("text"), str):
                raise ValueError("Invalid upstream response")
            value["text"].encode("utf-8")
        except urllib.error.HTTPError as error:
            # Upstream bodies and reason phrases may contain credentials.
            with error:
                phrase = self.responses.get(error.code, ("Request failed", ""))[0]
                self.error(error.code, f"Upstream HTTP {error.code}: {phrase}")
            return
        except (OSError, ValueError, http.client.HTTPException):
            self.error(502, "Invalid response from upstream speech service")
            return
        self.reply(200, {"text": value["text"]})


def main():
    parser = argparse.ArgumentParser(description="xAI transcription HTTP adapter")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL)
    args = parser.parse_args()
    try:
        with http.server.ThreadingHTTPServer((args.host, args.port), Handler) as server:
            server.base_url = args.base_url
            server.serve_forever()
    except Exception:
        parser.exit(1, "Could not run the xAI transcription adapter\n")
