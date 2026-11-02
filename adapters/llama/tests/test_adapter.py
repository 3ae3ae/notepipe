import base64
import http.client
import http.server
import io
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import Mock, patch
import urllib.error
import urllib.request
import wave

PACKAGE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGE / "src"))
from notepipe_asr_llama import DeadlineHTTPHandler, Handler, MAX_UPLOAD, sanitize_qwen_text


def form(audio=b"RIFF\0\xffWAVE", model=None, language=None):
    parts = []
    for name, value in [("model", model), ("language", language), ("file", audio)]:
        if value is None:
            continue
        value = value if isinstance(value, bytes) else value.encode()
        parts.append(f'--test\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'.encode() + value + b"\r\n")
    return b"".join(parts) + b"--test--\r\n"


class AdapterTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.record = Path(temporary.name)
        executable = self.record / "llama-server"
        executable.write_text(f"#!{sys.executable}\n" + (PACKAGE / "tests/fake_llama_server.py").read_text())
        executable.chmod(0o755)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            self.port = listener.getsockname()[1]
        self.process = subprocess.Popen([
            sys.executable, "-c", "from notepipe_asr_llama import main; main()",
            "--host", "127.0.0.1", "--port", str(self.port),
            "--llama-server", str(executable),
            "--record", str(self.record), "-hf", "example/model:Q8_0", "-ngl", "0",
        ], env={**os.environ, "PYTHONPATH": str(PACKAGE / "src"),
                "http_proxy": "http://127.0.0.1:1", "HTTP_PROXY": "http://127.0.0.1:1", "no_proxy": "", "NO_PROXY": ""},
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        self.addCleanup(self.stop)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                self.fail("adapter exited during startup: " + self.process.communicate()[1].decode())
            try:
                if self.call("GET", "/health")[0] == 503 and self.record.joinpath("pid").exists():
                    break
            except OSError:
                pass
            time.sleep(0.02)
        else:
            self.fail("adapter did not start")

    def stop(self):
        if self.process.poll() is None:
            self.process.terminate()
        try:
            self.process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.communicate()
            self.fail("adapter did not stop")

    def call(self, method="POST", path="/v1/audio/transcriptions", body=None, headers=None):
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=5)
        try:
            connection.request(method, path, body, {"Content-Type": "multipart/form-data; boundary=test", **(headers or {})})
            response = connection.getresponse()
            self.assertEqual(response.getheader("Content-Type"), "application/json")
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def response(self, body=None, **config):
        if body is not None:
            config["body"] = body
        self.record.joinpath("response.json").write_text(json.dumps(config))

    def test_health_tracks_child_readiness_and_arguments_pass_through(self):
        self.assertEqual(self.call("GET", "/health")[0], 503)
        self.record.joinpath("ready").touch()
        self.assertEqual(self.call("GET", "/health"), (200, {"status": "ok"}))
        self.record.joinpath("ready").unlink()
        self.assertEqual(self.call("GET", "/health")[0], 503)
        args = json.loads(self.record.joinpath("args.json").read_text())
        self.assertEqual(args[:-4], ["--record", str(self.record), "-hf", "example/model:Q8_0", "-ngl", "0"])
        self.assertEqual(args[-4:-1], ["--host", "127.0.0.1", "--port"])
        self.assertNotEqual(int(args[-1]), self.port)

    def test_chat_mapping_uses_exact_audio_model_and_language_code(self):
        audio = b"RIFF\0\xff\r\n--test-not-a-boundary\r\nWAVE"
        self.assertEqual(self.call(body=form(audio, "model", "ko"), headers={"Authorization": "Bearer secret"}), (200, {"text": "Recognized."}))
        request = json.loads(self.record.joinpath("request.json").read_text())
        self.assertEqual(request["path"], "/v1/chat/completions")
        self.assertEqual(request["headers"]["Content-Type"], "application/json")
        self.assertNotIn("Authorization", request["headers"])
        self.assertEqual(request["body"], {
            "model": "model", "temperature": 0,
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "Transcribe the audio. The speech is in ko."},
                {"type": "input_audio", "input_audio": {"format": "wav", "data": base64.b64encode(audio).decode()}},
            ]}],
        })

    def test_optional_fields_and_marker_cleanup(self):
        for text, expected in [
            ("language English<asr_text>The blue notebook.</asr_text>tail", "The blue notebook."),
            ("Language models help.", "Language models help."), ("", ""),
        ]:
            with self.subTest(text=text):
                self.response(json.dumps({"choices": [{"message": {"content": text}}]}))
                self.assertEqual(self.call(body=form(model="", language="")), (200, {"text": expected}))
                request = json.loads(self.record.joinpath("request.json").read_text())["body"]
                self.assertNotIn("model", request)
                self.assertEqual(request["messages"][0]["content"][0]["text"], "Transcribe the audio.")

    def test_upstream_errors_are_json_and_redirects_are_not_followed(self):
        for status in [302, 400, 401, 429, 500, 503, 599]:
            with self.subTest(status=status):
                self.response("untrusted-secret", status=status, location="http://127.0.0.1:1/leak")
                code, value = self.call(body=form())
                self.assertEqual(code, status)
                self.assertIn(str(status), value["error"]["message"])
                self.assertNotIn("untrusted-secret", json.dumps(value))

    def test_invalid_upstream_content_is_a_gateway_error(self):
        for body in ["not JSON", "null", "[]", "{}", '{"choices":[]}', '{"choices":[null]}',
                     '{"choices":[{"message":{"content":null}}]}',
                     '{"choices":[{"message":{"content":42}}]}',
                     r'{"choices":[{"message":{"content":"\ud800"}}]}',
                     '{"error":{"message":"untrusted-secret"}}']:
            with self.subTest(body=body):
                self.response(body)
                code, value = self.call(body=form())
                self.assertEqual(code, 502)
                self.assertIsInstance(value["error"]["message"], str)

    def requests(self):
        return [json.loads(line) for line in self.record.joinpath("requests.jsonl").read_text().splitlines()]

    def test_output_cap_follows_the_audio_length(self):
        audio = io.BytesIO()
        with wave.open(audio, "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(16000)
            wav.writeframes(b"\0\0" * 16000 * 3)
        self.assertEqual(self.call(body=form(audio.getvalue())), (200, {"text": "Recognized."}))
        self.assertEqual(self.requests()[0]["max_tokens"], 40 + 25 * 3)

    def test_a_segment_at_its_cap_is_decoded_again_with_fallbacks(self):
        repeating = json.dumps({"choices": [{"finish_reason": "length", "message": {"content": "시, 시, 시,"}}]})
        self.record.joinpath("response.json").write_text(json.dumps([
            {"body": repeating}, {"body": repeating},
            {"body": json.dumps({"choices": [{"finish_reason": "stop", "message": {"content": "시련이죠?"}}]})},
        ]))
        self.assertEqual(self.call(body=form()), (200, {
            "text": "시련이죠?",
            "warning": "The first decoding reached the output cap; decoded again with a repetition penalty and sampling at temperature 0.4.",
        }))
        self.assertEqual([(r["temperature"], r.get("repeat_penalty")) for r in self.requests()],
                         [(0, None), (0, 1.1), (0.4, 1.1)])

    def test_first_fallback_reports_a_warning(self):
        self.record.joinpath("response.json").write_text(json.dumps([
            {"body": json.dumps({"choices": [{"finish_reason": "length", "message": {"content": "repeat"}}]})},
            {"body": json.dumps({"choices": [{"finish_reason": "stop", "message": {"content": "Done."}}]})},
        ]))
        self.assertEqual(self.call(body=form()), (200, {
            "text": "Done.",
            "warning": "The first decoding reached the output cap; decoded again with a repetition penalty.",
        }))
        self.assertEqual(len(self.requests()), 2)

    def test_output_limit_is_a_failure_that_is_not_retried(self):
        self.response(json.dumps({"choices": [{"finish_reason": "length", "message": {"content": "The blue"}}]}))
        code, value = self.call(body=form())
        self.assertEqual(code, 422)
        self.assertEqual(value["error"]["code"], "output_limit")
        self.assertIn("output limit", value["error"]["message"])
        self.assertEqual(len(self.requests()), 3, "every fallback is tried first")

    def test_malformed_uploads_and_unknown_routes_are_json_errors(self):
        for method, path, body, headers, status in [
            ("GET", "/unknown", None, {}, 404),
            ("POST", "/unknown", b"", {}, 404),
            ("PUT", "/v1/audio/transcriptions", b"", {}, 501),
            ("POST", "/v1/audio/transcriptions", b"invalid", {}, 400),
            ("POST", "/v1/audio/transcriptions", form(None, "model"), {}, 400),
            ("POST", "/v1/audio/transcriptions", form()[:-8], {}, 400),
            ("POST", "/v1/audio/transcriptions", form(), {"Content-Type": "application/json"}, 400),
            ("POST", "/v1/audio/transcriptions", form(), {"Content-Type": "multipart/form-data"}, 400),
            ("POST", "/v1/audio/transcriptions", form(model=b"\xff"), {}, 400),
            ("POST", "/v1/audio/transcriptions", form()[:-10] + form(), {}, 400),
            ("POST", "/v1/audio/transcriptions", b"", {"Content-Length": "invalid"}, 400),
            ("POST", "/v1/audio/transcriptions", b"", {"Content-Length": "-1"}, 411),
            ("POST", "/v1/audio/transcriptions", b"", {"Content-Length": str(MAX_UPLOAD + 1)}, 413),
        ]:
            with self.subTest(method=method, path=path, status=status):
                code, value = self.call(method, path, body, headers)
                self.assertEqual(code, status)
                self.assertIsInstance(value["error"]["message"], str)
        self.assertFalse(self.record.joinpath("request.json").exists())

    def test_sigterm_stops_child(self):
        self.process.terminate()
        self.process.communicate(timeout=10)
        self.assertEqual(self.process.returncode, 0)
        self.assertTrue(self.record.joinpath("terminated").exists())

    def test_sigint_stops_child(self):
        self.process.send_signal(signal.SIGINT)
        self.process.communicate(timeout=10)
        self.assertEqual(self.process.returncode, 0)
        self.assertTrue(self.record.joinpath("terminated").exists())

    def test_unresponsive_child_is_killed(self):
        self.record.joinpath("ignore-term").touch()
        self.process.terminate()
        self.process.communicate(timeout=10)
        self.assertEqual(self.process.returncode, 0)
        pid = int(self.record.joinpath("pid").read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)

    def test_child_exit_is_fatal_with_last_stderr_lines(self):
        self.record.joinpath("exit").write_text("7")
        _, stderr = self.process.communicate(timeout=10)
        self.assertNotEqual(self.process.returncode, 0)
        self.assertIn(b"llama-server exited with status 7", stderr)
        self.assertIn(b"child diagnostic 29", stderr)
        self.assertNotIn(b"child diagnostic 0\n", stderr)


class DeadlineTests(unittest.TestCase):
    def setUp(self):
        self.handler = object.__new__(Handler)
        self.handler.server = Mock(base_url="http://127.0.0.1:1234")
        body = form()
        self.handler.headers = {"Content-Length": str(len(body)), "Content-Type": "multipart/form-data; boundary=test"}
        self.handler.rfile = io.BytesIO(body)
        self.handler.path = "/v1/audio/transcriptions"
        self.handler.reply = Mock()
        opener = patch("notepipe_asr_llama.urllib.request.build_opener")
        self.addCleanup(opener.stop)
        self.opener = opener.start().return_value
        clock = patch("notepipe_asr_llama.monotonic")
        self.addCleanup(clock.stop)
        self.clock = clock.start()

    def responses(self, reasons):
        responses = []
        for reason in reasons:
            response = Mock()
            response.__enter__ = Mock(return_value=response)
            response.__exit__ = Mock(return_value=False)
            response.read.return_value = json.dumps({"choices": [{"finish_reason": reason, "message": {"content": "Done."}}]}).encode()
            responses.append(response)
        self.opener.open.side_effect = responses

    def assert_timeout(self):
        self.handler.reply.assert_called_once_with(504, {"error": {"message": "The speech model did not finish within the time limit"}})

    def test_decodes_share_one_570_second_budget(self):
        self.responses(["length", "length", "stop"])
        self.clock.side_effect = [100, 100, 200, 200, 400, 400, 500]
        self.handler.do_POST()
        self.assertEqual([call.kwargs["timeout"] for call in self.opener.open.call_args_list], [570, 470, 270])
        self.assertEqual(self.handler.reply.call_args.args[0], 200)

    def test_expired_budget_stops_before_another_decode(self):
        self.responses(["length"])
        self.clock.side_effect = [100, 100, 671]
        self.handler.do_POST()
        self.assert_timeout()
        self.assertEqual(self.opener.open.call_count, 1)

    def test_a_late_success_is_a_timeout(self):
        self.responses(["stop"])
        self.clock.side_effect = [100, 100, 671]
        self.handler.do_POST()
        self.assert_timeout()

    def test_upstream_timeouts_are_504(self):
        for error in [TimeoutError(), urllib.error.URLError(TimeoutError())]:
            with self.subTest(error=error):
                self.handler.rfile.seek(0)
                self.handler.reply.reset_mock()
                self.clock.return_value = 100
                self.opener.open.side_effect = error
                self.handler.do_POST()
                self.assert_timeout()


class TransportDeadlineTests(unittest.TestCase):
    def test_deadline_interrupts_a_trickling_response(self):
        class Trickling(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Length", "20")
                self.end_headers()
                try:
                    for _ in range(20):
                        self.wfile.write(b"x")
                        self.wfile.flush()
                        time.sleep(0.05)
                except OSError:
                    pass

            def log_message(self, *_):
                pass

        with http.server.ThreadingHTTPServer(("127.0.0.1", 0), Trickling) as server:
            thread = threading.Thread(target=server.serve_forever)
            thread.start()
            started = time.monotonic()
            upstream = DeadlineHTTPHandler(started + 0.2)
            try:
                opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), upstream)
                with self.assertRaises((OSError, http.client.HTTPException)):
                    with opener.open(f"http://127.0.0.1:{server.server_port}", timeout=0.2) as response:
                        response.read()
                self.assertLess(time.monotonic() - started, 0.8)
            finally:
                if upstream.timer is not None:
                    upstream.timer.cancel()
                server.shutdown()
                thread.join()


class CleanupTests(unittest.TestCase):
    def test_exact_former_core_cases(self):
        for text, expected in [
            ("**language Chinese<asr_text>你好世界。", "你好世界。"),
            ("内容</asr_text>尾巴", "内容"),
            ("Language models help.", "Language models help."),
            ("language Chinese\u3000你好", "language Chinese\u3000你好"),
            ("language English<asr_text>Language models help.</asr_text>", "Language models help."),
            (" <asr_text>prefix<asr_text> final </asr_text>tail ", "final"),
        ]:
            with self.subTest(text=text):
                self.assertEqual(sanitize_qwen_text(text), expected)


if __name__ == "__main__":
    unittest.main()
