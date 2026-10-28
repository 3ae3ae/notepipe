import http.client
import io
from pathlib import Path
import sys
import threading
import unittest
from unittest.mock import Mock, patch
import wave

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from notepipe_asr_sensevoice import Handler, MAX_UPLOAD, Model, multipart
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


def wav_audio(channels=1, width=2, rate=16000, frames=b"\0\0" * 160):
    audio = io.BytesIO()
    with wave.open(audio, "wb") as wav:
        wav.setnchannels(channels)
        wav.setsampwidth(width)
        wav.setframerate(rate)
        wav.writeframes(frames)
    return audio.getvalue()


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
            server.model = Model.__new__(Model)
            server.model.name = "model"
            server.model.pipe = Mock()
            server.model.pipe.generate.return_value = [{"text": "<|en|>Hello."}]
            for language in [None, "auto", "ko", "zh-TW"]:
                self.assertEqual(call("POST", "/v1/audio/transcriptions", form(wav_audio(), language=language)), (200, {"text": "Hello."}))
            code, result = call("POST", "/v1/audio/transcriptions", form(wav_audio(), language="de"))
            self.assertEqual(code, 400)
            self.assertIn("zh, en, ja, ko, yue", result["error"]["message"])
            self.assertIn("leaving language unset", result["error"]["message"])
            self.assertEqual(call("POST", "/v1/audio/transcriptions", form(b"not WAV"))[0], 400)
        finally:
            server.shutdown()
            serving.join()
            server.server_close()

    def test_model_device_defaults_and_explicit_devices(self):
        torch, funasr = Mock(), Mock()
        for device, cuda, mps, expected in [
            (None, True, True, "cuda"), (None, False, True, "mps"),
            (None, False, False, "cpu"), ("cpu", True, True, "cpu"),
            ("cuda:1", True, True, "cuda:1"),
        ]:
            with self.subTest(device=device, cuda=cuda, mps=mps):
                torch.cuda.is_available.return_value = cuda
                torch.backends.mps.is_available.return_value = mps
                with patch.dict(sys.modules, {"torch": torch, "funasr": funasr}):
                    model = Model("model", device)
                self.assertEqual(model.name, "model")
                self.assertEqual(model.device, expected)
                funasr.AutoModel.assert_called_with(model="model", hub="hf", device=expected, disable_update=True, disable_pbar=True)

    def test_language_and_float32_samples_are_passed_to_generate(self):
        import numpy as np

        model = Model.__new__(Model)
        model.pipe = Mock()
        model.pipe.generate.return_value = [{"text": " result "}]
        audio = wav_audio(frames=b"\0\x80\0\0\xff\x7f")
        for language, expected in [
            (None, "auto"), ("", "auto"), ("auto", "auto"), ("zh", "zh"),
            ("en", "en"), ("ja", "ja"), ("ko", "ko"), ("yue", "yue"),
            ("zh-TW", "zh"), ("EN-us", "en"), ("KO-kr", "ko"),
        ]:
            with self.subTest(language=language):
                self.assertEqual(model.transcribe(audio, language), "result")
                kwargs = model.pipe.generate.call_args.kwargs
                self.assertEqual(set(kwargs), {"input", "cache", "language", "use_itn"})
                self.assertEqual(kwargs["language"], expected)
                self.assertEqual(kwargs["cache"], {})
                self.assertIs(kwargs["use_itn"], True)
                self.assertEqual(kwargs["input"].dtype, np.float32)
                np.testing.assert_array_equal(kwargs["input"], [-1.0, 0.0, 32767 / 32768])
        model.pipe.generate.reset_mock()
        with self.assertRaisesRegex(ValueError, "zh, en, ja, ko, yue.*leaving language unset"):
            model.transcribe(audio, "de")
        model.pipe.generate.assert_not_called()

    def test_tags_are_removed_without_emojis_and_empty_speech_is_empty(self):
        model = Model.__new__(Model)
        model.pipe = Mock()
        for result, expected in [
            ([{"text": " <|en|><|NEUTRAL|><|Speech|><|withitn|>Hello. "}], "Hello."),
            ([{"text": "<|ko|><|HAPPY|><|Laughter|><|unknown\ntag|>"}], ""),
            ([{"text": "Hello<|Speech|> world."}], "Hello world."),
            ([{"text": ""}], ""), ([], ""),
        ]:
            with self.subTest(result=result):
                model.pipe.generate.return_value = result
                self.assertEqual(model.transcribe(wav_audio(), ""), expected)

    def test_wav_validation_rejects_wrong_format_empty_and_truncated_audio(self):
        model = Model.__new__(Model)
        model.pipe = Mock()
        for audio in [
            b"not WAV", wav_audio(channels=2), wav_audio(width=1),
            wav_audio(rate=8000), wav_audio(frames=b""), wav_audio()[:-2],
        ]:
            with self.subTest(audio=audio[:44]), self.assertRaises((ValueError, wave.Error, EOFError)):
                model.transcribe(audio, "")
        model.pipe.generate.assert_not_called()


if __name__ == "__main__":
    unittest.main()
