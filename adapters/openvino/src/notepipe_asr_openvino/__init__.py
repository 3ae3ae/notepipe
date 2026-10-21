"""A single loaded OpenVINO model behind the transcription HTTP contract."""
import argparse
from email import policy
from email.parser import BytesParser
import http.server
import io
import json
from pathlib import Path
import sys
import time
import wave

MAX_UPLOAD = 64 * 1024 * 1024
DEFAULT_MODEL = "dseditor/Qwen3-ASR-1.7B-INT8_OpenVINO"
# Qwen's generation API takes language names; the HTTP contract takes codes.
QWEN_LANGUAGES = {
    "zh": "Chinese", "yue": "Cantonese", "en": "English", "de": "German",
    "fr": "French", "es": "Spanish", "pt": "Portuguese", "ru": "Russian",
    "it": "Italian", "nl": "Dutch", "ar": "Arabic", "ko": "Korean",
    "ja": "Japanese", "hi": "Hindi", "ms": "Malay", "vi": "Vietnamese",
    "th": "Thai", "id": "Indonesian", "tr": "Turkish", "fa": "Persian",
    "sv": "Swedish", "da": "Danish", "fi": "Finnish", "pl": "Polish",
    "cs": "Czech", "fil": "Filipino", "tl": "Filipino", "hu": "Hungarian",
    "el": "Greek", "ro": "Romanian", "mk": "Macedonian",
}


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


class Model:
    def __init__(self, model, device):
        import openvino_genai as ov_genai
        from huggingface_hub import snapshot_download

        self.name = model
        self.ov = ov_genai
        self.path = Path(model) if Path(model).is_dir() else Path(snapshot_download(model))
        config = self.path / "config.json"
        model_type = json.loads(config.read_text()).get("model_type", "") if config.is_file() else ""
        self.whisper = model_type == "whisper" or "whisper" in model.lower()
        if self.whisper:
            self.pipe = ov_genai.WhisperPipeline(str(self.path), device)
        else:
            if not hasattr(ov_genai, "ASRPipeline"):
                raise RuntimeError("This OpenVINO GenAI version does not support ASRPipeline; select a Whisper model explicitly")
            self.pipe = ov_genai.ASRPipeline(str(self.path), device)

    def transcribe(self, audio, language):
        import numpy as np

        with wave.open(io.BytesIO(audio), "rb") as wav:
            if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate(), wav.getcomptype()) != (1, 2, 16000, "NONE"):
                raise ValueError("Expected a 16 kHz mono 16-bit PCM WAV")
            frames = wav.readframes(wav.getnframes())
            if len(frames) != wav.getnframes() * 2 or not frames:
                raise ValueError("Empty or truncated WAV")
        samples = np.frombuffer(frames, dtype="<i2").astype(np.float32) / 32768.0
        if self.whisper:
            config_path = self.path / "generation_config.json"
            config = self.ov.WhisperGenerationConfig(str(config_path)) if config_path.is_file() else self.ov.WhisperGenerationConfig()
            if config.is_multilingual:
                config.task = "transcribe"
                config.language = f"<|{language}|>" if language else None
            elif language and language != "en":
                raise ValueError("This English-only Whisper model cannot transcribe the requested language")
        else:
            config = self.pipe.get_generation_config()
            config.language = QWEN_LANGUAGES.get(language, language) if language else None
        result = self.pipe.generate(samples.tolist(), config)
        return result.texts[0].strip() if result.texts else ""


class Handler(http.server.BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(120)

    def send_error(self, code, message=None, explain=None):
        self.error(code, message or "Unsupported request")

    def log_message(self, format, *args):
        print(format % args, file=sys.stderr)

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
            model = fields.get("model", "")
            if model and model != self.server.model.name:
                raise ValueError("Requested model does not match the loaded model")
            text = self.server.model.transcribe(fields["file"], fields.get("language", ""))
            self.reply(200, {"text": text})
        except (ValueError, UnicodeError, wave.Error, EOFError) as error:
            self.error(400, str(error))
        except Exception as error:
            self.error(500, str(error))


def main():
    parser = argparse.ArgumentParser(description="OpenVINO transcription HTTP adapter")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--device", choices=["NPU", "CPU", "GPU"], default="NPU")
    args = parser.parse_args()
    try:
        started = time.monotonic()
        print(f"Loading {args.model} on {args.device}", file=sys.stderr, flush=True)
        model = Model(args.model, args.device)
        # Do not bind or advertise readiness until the selected model is loaded.
        server = http.server.HTTPServer((args.host, args.port), Handler)
        server.model = model
        print(f"Ready on {args.host}:{args.port} after {time.monotonic() - started:.1f}s", file=sys.stderr, flush=True)
        with server:
            server.serve_forever()
    except Exception as error:
        parser.exit(1, f"{error}\n")
