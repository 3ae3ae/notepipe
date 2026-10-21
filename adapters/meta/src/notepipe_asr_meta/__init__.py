"""Forward the notepipe transcription contract to Meta's speech endpoint."""
import argparse
from email import policy
from email.parser import BytesParser
import http.client
import http.server
from http import HTTPStatus
import json
from urllib.parse import urlsplit
import uuid

MAX_UPLOAD = 64 * 1024 * 1024
DEFAULT_BASE_URL = "https://api.meta.ai/v1"
LANGUAGE_NAMES = {
    "af": "Afrikaans", "am": "Amharic", "ar": "Arabic", "az": "Azerbaijani",
    "bg": "Bulgarian", "bn": "Bengali", "cs": "Czech", "da": "Danish",
    "de": "German", "el": "Greek", "en": "English", "es": "Spanish",
    "fa": "Persian", "fi": "Finnish", "fr": "French", "gu": "Gujarati",
    "ha": "Hausa", "he": "Hebrew", "hi": "Hindi", "hu": "Hungarian",
    "id": "Indonesian", "it": "Italian", "ja": "Japanese", "jv": "Javanese",
    "kn": "Kannada", "ko": "Korean", "ml": "Malayalam", "mr": "Marathi",
    "ms": "Malay", "my": "Burmese", "ne": "Nepali", "nl": "Dutch",
    "no": "Norwegian", "pa": "Punjabi", "pl": "Polish", "ps": "Pashto",
    "pt": "Portuguese", "ro": "Romanian", "ru": "Russian", "si": "Sinhala",
    "so": "Somali", "sr": "Serbian", "su": "Sundanese", "sv": "Swedish",
    "sw": "Swahili", "ta": "Tamil", "te": "Telugu", "th": "Thai",
    "tl": "Tagalog", "tr": "Turkish", "uk": "Ukrainian", "ur": "Urdu",
    "uz": "Uzbek", "vi": "Vietnamese", "yo": "Yoruba", "zh": "Mandarin Chinese",
    # Speech bias names the spoken language; Meta's docs call it "Mandarin Chinese" whatever the script.
    "zh-hans": "Mandarin Chinese", "zh-hant": "Mandarin Chinese",
}


def english_name(code):
    wanted = code.strip().replace("_", "-").lower()
    while wanted:
        if wanted in LANGUAGE_NAMES:
            return LANGUAGE_NAMES[wanted]
        wanted = wanted.rpartition("-")[0]
    return code


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
    request = {"audioEncoding": "WAV"}
    if fields.get("model"):
        request["model"] = fields["model"]
    if fields.get("language"):
        request["languageBias"] = [english_name(fields["language"])]
    boundary = f"notepipe-{uuid.uuid4().hex}"
    body = (
        f'--{boundary}\r\nContent-Disposition: form-data; name="request"\r\n'
        'Content-Type: application/json\r\n\r\n'
        f'{json.dumps(request, ensure_ascii=False)}\r\n'
        f'--{boundary}\r\nContent-Disposition: form-data; name="audio"; filename="audio.wav"\r\n'
        'Content-Type: audio/wav\r\n\r\n'
    ).encode() + fields["file"] + f"\r\n--{boundary}--\r\n".encode()
    return f"multipart/form-data; boundary={boundary}", body


def error(message):
    return {"error": {"message": message}}


def transcribe(base_url, fields, authorization):
    connection = None
    try:
        url = urlsplit(base_url.rstrip("/") + "/asr/transcribe")
        if url.scheme not in {"http", "https"} or not url.hostname or url.username or url.password or url.query or url.fragment:
            raise ValueError("Invalid base URL")
        content_type, body = transcription_request(fields)
        headers = {"Content-Type": content_type}
        if authorization is not None:
            headers["Authorization"] = authorization
        connection_type = http.client.HTTPSConnection if url.scheme == "https" else http.client.HTTPConnection
        connection = connection_type(url.hostname, url.port, timeout=120)
        # http.client never follows redirects, keeping credentials on this endpoint.
        connection.request("POST", url.path, body, headers)
        response = connection.getresponse()
        if not 200 <= response.status < 300:
            try:
                phrase = HTTPStatus(response.status).phrase
            except ValueError:
                phrase = "HTTP error"
            return response.status, error(f"Upstream returned {response.status} {phrase}")
        raw = response.read(MAX_UPLOAD + 1)
        if len(raw) > MAX_UPLOAD:
            return 502, error("Invalid upstream transcription response")
        value = json.loads(raw)
        if not isinstance(value, dict) or not isinstance(value.get("transcript"), str) or value.get("error") is not None:
            return 502, error("Invalid upstream transcription response")
        value["transcript"].encode("utf-8")
        return 200, {"text": value["transcript"]}
    except (ValueError, UnicodeError):
        return 502, error("Invalid upstream transcription response")
    except (OSError, http.client.HTTPException):
        return 502, error("Could not reach upstream transcription service")
    finally:
        if connection is not None:
            connection.close()


class Handler(http.server.BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(120)

    def log_message(self, format, *args):
        pass

    def send_error(self, code, message=None, explain=None):
        self.reply(code, error("Unsupported request"))

    def reply(self, status, value):
        body = json.dumps(value, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if self.path == "/health":
            self.reply(200, {"status": "ok"})
        else:
            self.reply(404, error("Unknown endpoint"))

    def do_POST(self):
        if self.path != "/v1/audio/transcriptions":
            self.reply(404, error("Unknown endpoint"))
            return
        try:
            length = int(self.headers.get("Content-Length", "-1"))
            if length < 0:
                self.reply(411, error("Content-Length is required"))
                return
            if length > MAX_UPLOAD:
                self.reply(413, error("Audio upload exceeds 64 MiB"))
                return
            body = self.rfile.read(length)
            if len(body) != length:
                raise ValueError("Truncated upload")
            fields = multipart(self.headers.get("Content-Type", ""), body)
        except (ValueError, UnicodeError, OSError):
            self.reply(400, error("Invalid transcription request"))
            return
        status, value = transcribe(self.server.base_url, fields, self.headers.get("Authorization"))
        self.reply(status, value)


def main():
    parser = argparse.ArgumentParser(description="Meta transcription HTTP adapter")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--base-url", default=DEFAULT_BASE_URL)
    args = parser.parse_args()
    try:
        with http.server.ThreadingHTTPServer((args.host, args.port), Handler) as server:
            server.base_url = args.base_url
            server.serve_forever()
    except (OSError, ValueError):
        parser.exit(1, "Could not start transcription adapter\n")
