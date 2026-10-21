import http.client
import io
from pathlib import Path
import sys
import threading
import unittest
from unittest.mock import Mock
import wave

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from notepipe_asr_openvino import Handler, MAX_UPLOAD, Model, multipart
import http.server
import json


def form(audio, model=None, language=None):
    parts = []
    for name, value in [("model", model), ("language", language), ("file", audio)]:
        if value is None:
            continue
        value = value if isinstance(value, bytes) else value.encode()
        parts.append(f'--test\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'.encode() + value + b"\r\n")
    return b"".join(parts) + b"--test--\r\n"


class AdapterTests(unittest.TestCase):
    def test_multipart_preserves_binary_and_optional_fields(self):
        audio = b"RIFF\0\xff\r\n--test-not-a-boundary\r\nWAVE"
        fields = multipart('multipart/form-data; boundary="test"', form(audio, "model", "ko"))
        self.assertEqual(fields, {"model": "model", "language": "ko", "file": audio})
        self.assertEqual(multipart("multipart/form-data; boundary=test", form(audio)), {"file": audio})

    def test_multipart_rejects_missing_duplicate_and_truncated_parts(self):
        for content_type, body in [
            ("application/json", b"{}"),
            ("multipart/form-data", form(b"audio")),
            ("multipart/form-data; boundary=test", form(None, model="model")),
            ("multipart/form-data; boundary=test", form(b"audio")[:-8]),
            ("multipart/form-data; boundary=test", form(b"a")[:-10] + form(b"b")),
        ]:
            with self.subTest(content_type=content_type, body=body), self.assertRaises(ValueError):
                multipart(content_type, body)

    def test_http_contract_and_json_errors(self):
        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        server.model = Mock(name="loaded_model")
        server.model.name = "model"
        server.model.transcribe.return_value = "Recognized"
        serving = threading.Thread(target=server.serve_forever)
        serving.start()
        try:
            def call(method, path, body=None, extra_headers=None):
                connection = http.client.HTTPConnection(*server.server_address, timeout=5)
                headers = {"Content-Type": "multipart/form-data; boundary=test", **(extra_headers or {})}
                connection.request(method, path, body, headers)
                response = connection.getresponse()
                self.assertEqual(response.getheader("Content-Type"), "application/json")
                result = response.status, json.loads(response.read())
                connection.close()
                return result

            self.assertEqual(call("GET", "/health")[0], 200)
            self.assertEqual(call("POST", "/v1/audio/transcriptions", form(b"audio", "model", "ko")), (200, {"text": "Recognized"}))
            server.model.transcribe.assert_called_once_with(b"audio", "ko")
            for method, path, body, status in [
                ("POST", "/v1/audio/transcriptions", form(b"audio", "different"), 400),
                ("POST", "/v1/audio/transcriptions", b"invalid", 400),
                ("GET", "/unknown", None, 404),
                ("PUT", "/v1/audio/transcriptions", b"", 501),
            ]:
                code, result = call(method, path, body)
                self.assertEqual(code, status)
                self.assertIsInstance(result["error"]["message"], str)
            self.assertEqual(call("POST", "/v1/audio/transcriptions", b"", {"Content-Length": str(MAX_UPLOAD + 1)})[0], 413)
            server.model.transcribe.side_effect = RuntimeError("inference failed")
            self.assertEqual(call("POST", "/v1/audio/transcriptions", form(b"audio")), (500, {"error": {"message": "inference failed"}}))
        finally:
            server.shutdown()
            serving.join()
            server.server_close()

    def test_whisper_language_is_reset_for_each_request(self):
        # Real generation-config and WAV decoding; only inference is replaced.
        try:
            import openvino_genai as ov
        except ImportError:
            self.skipTest("OpenVINO is only installed in the Linux verification environment")
        model = Model.__new__(Model)
        model.path = Path("/nonexistent-model")
        model.whisper = True
        model.ov = ov
        model.pipe = Mock()
        model.pipe.generate.return_value.texts = [" result "]
        audio = io.BytesIO()
        with wave.open(audio, "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(16000)
            wav.writeframes(b"\0\0" * 160)
        self.assertEqual(model.transcribe(audio.getvalue(), "ko"), "result")
        self.assertEqual(model.pipe.generate.call_args.args[1].language, "<|ko|>")
        model.transcribe(audio.getvalue(), "")
        self.assertIsNone(model.pipe.generate.call_args.args[1].language)
        with self.assertRaises((ValueError, wave.Error, EOFError)):
            model.transcribe(b"not WAV", "")

    def test_qwen_language_codes_become_model_language_names(self):
        try:
            import openvino_genai as ov
        except ImportError:
            self.skipTest("OpenVINO is only installed in the Linux verification environment")
        model = Model.__new__(Model)
        model.whisper = False
        model.pipe = Mock()
        model.pipe.get_generation_config.side_effect = ov.ASRGenerationConfig
        model.pipe.generate.return_value.texts = ["result"]
        audio = (Path(__file__).resolve().parents[3] / "tests/fixtures/speech.wav").read_bytes()
        model.transcribe(audio, "ko")
        self.assertEqual(model.pipe.generate.call_args.args[1].language, "Korean")
        model.transcribe(audio, "")
        self.assertIsNone(model.pipe.generate.call_args.args[1].language)

    def test_language_subtags_use_primary_code_case_insensitively(self):
        model = Model.__new__(Model)
        model.path = Path("/nonexistent-model")
        model.ov = Mock()
        model.ov.WhisperGenerationConfig.return_value.is_multilingual = True
        model.pipe = Mock()
        model.pipe.generate.return_value.texts = ["result"]
        audio = (Path(__file__).resolve().parents[3] / "tests/fixtures/speech.wav").read_bytes()
        for whisper in [True, False]:
            model.whisper = whisper
            for language, primary, name in [
                ("pt-BR", "pt", "Portuguese"), ("pT-br", "pt", "Portuguese"),
                ("en-US", "en", "English"), ("EN-us", "en", "English"),
                ("zh-Hant", "zh", "Chinese"), ("ZH-hANT", "zh", "Chinese"),
            ]:
                with self.subTest(whisper=whisper, language=language):
                    model.transcribe(audio, language)
                    expected = f"<|{primary}|>" if whisper else name
                    self.assertEqual(model.pipe.generate.call_args.args[1].language, expected)
        model.whisper = True
        model.ov.WhisperGenerationConfig.return_value.is_multilingual = False
        self.assertEqual(model.transcribe(audio, "EN-us"), "result")
        with self.assertRaisesRegex(ValueError, "English-only Whisper model"):
            model.transcribe(audio, "pt-BR")


if __name__ == "__main__":
    unittest.main()
