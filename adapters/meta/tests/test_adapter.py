from concurrent.futures import ThreadPoolExecutor
from contextlib import redirect_stderr
from email import policy
from email.parser import BytesParser
import http.client
import http.server
import io
import json
from pathlib import Path
import sys
import threading
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from notepipe_asr_meta import Handler, MAX_UPLOAD, english_name, multipart


def form(audio=b"RIFF\0\xffWAVE", model=None, language=None):
    parts = []
    for name, value in [("model", model), ("language", language), ("file", audio)]:
        if value is not None:
            value = value if isinstance(value, bytes) else value.encode()
            parts.append(f'--test\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'.encode() + value + b"\r\n")
    return b"".join(parts) + b"--test--\r\n"


class Upstream(http.server.BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        self.server.requests.append((self.path, self.headers, body))
        if self.server.barrier is not None:
            self.server.barrier.wait(timeout=5)
        self.send_response(self.server.status, self.server.reason)
        if self.server.location:
            self.send_header("Location", self.server.location)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(self.server.body)))
        self.end_headers()
        self.wfile.write(self.server.body)


class AdapterTests(unittest.TestCase):
    def setUp(self):
        self.upstream = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
        self.upstream.requests = []
        self.upstream.body = b'{"transcript":"Recognized"}'
        self.upstream.status = 200
        self.upstream.reason = None
        self.upstream.location = None
        self.upstream.barrier = None
        self.adapter = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.adapter.base_url = f"http://127.0.0.1:{self.upstream.server_port}/v1/"
        self.threads = []
        for server in (self.upstream, self.adapter):
            thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01})
            thread.start()
            self.threads.append(thread)

    def tearDown(self):
        for server in (self.adapter, self.upstream):
            server.shutdown()
            server.server_close()
        for thread in self.threads:
            thread.join()

    def call(self, method="POST", path="/v1/audio/transcriptions", body=None, headers=None):
        connection = http.client.HTTPConnection(*self.adapter.server_address, timeout=10)
        connection.request(method, path, body, {"Content-Type": "multipart/form-data; boundary=test", **(headers or {})})
        response = connection.getresponse()
        self.assertEqual(response.getheader("Content-Type"), "application/json")
        result = response.status, json.loads(response.read())
        connection.close()
        return result

    def forwarded(self):
        path, headers, body = self.upstream.requests[-1]
        message = BytesParser(policy=policy.default).parsebytes(
            f'Content-Type: {headers["Content-Type"]}\r\n\r\n'.encode() + body
        )
        self.assertFalse(message.defects)
        return path, headers, list(message.iter_parts())

    def test_request_mapping_binary_audio_and_authorization(self):
        audio = b"RIFF\0\xff\r\n--test-not-a-boundary\r\nWAVE"
        authorization = "Bearer Exact-Case-sensitive-key"
        self.upstream.body = json.dumps({"transcript": "안녕하세요 世界"}, ensure_ascii=False).encode()
        self.assertEqual(self.call(body=form(audio, "muse-voice-transcribe-1.0", "ko"), headers={"Authorization": authorization}), (200, {"text": "안녕하세요 世界"}))
        path, headers, parts = self.forwarded()
        self.assertEqual(path, "/v1/asr/transcribe")
        self.assertEqual(headers["Authorization"], authorization)
        self.assertEqual([part.get_param("name", header="content-disposition") for part in parts], ["request", "audio"])
        self.assertEqual(parts[0].get_content_type(), "application/json")
        self.assertEqual(json.loads(parts[0].get_payload(decode=True)), {"audioEncoding": "WAV", "model": "muse-voice-transcribe-1.0", "languageBias": ["Korean"]})
        self.assertEqual(parts[1].get_content_type(), "audio/wav")
        self.assertEqual(parts[1].get_filename(), "audio.wav")
        self.assertEqual(parts[1].get_payload(decode=True), audio)

    def test_optional_fields_and_authorization_are_omitted(self):
        self.upstream.body = b'{"transcript":""}'
        for options in ({}, {"model": "", "language": ""}):
            with self.subTest(options=options):
                self.assertEqual(self.call(body=form(**options)), (200, {"text": ""}))
                _, headers, parts = self.forwarded()
                self.assertNotIn("Authorization", headers)
                self.assertEqual(json.loads(parts[0].get_payload(decode=True)), {"audioEncoding": "WAV"})

    def test_language_names_match_core_normalization_and_unknown_fallback(self):
        for code, expected in [
            ("ko", "Korean"), ("EN_us", "English"), (" PT_br ", "Portuguese"),
            ("zh", "Mandarin Chinese"), ("zh-Hans", "Mandarin Chinese"),
            ("ZH_hant_TW", "Mandarin Chinese"), ("sr-Latn-RS", "Serbian"),
            ("xx-custom", "xx-custom"), (" Unknown_name ", " Unknown_name "),
        ]:
            with self.subTest(code=code):
                self.assertEqual(english_name(code), expected)
                self.assertEqual(self.call(body=form(language=code))[0], 200)
                _, _, parts = self.forwarded()
                self.assertEqual(json.loads(parts[0].get_payload(decode=True))["languageBias"], [expected])

    def test_malformed_success_responses_are_bad_gateway(self):
        for body in [b"not JSON", b"\xff", b"[]", b"null", b"{}", b'{"transcript":null}', b'{"transcript":12}', b'{"transcript":"\\ud800"}', b'{"transcript":"text","error":{"message":"secret"}}']:
            with self.subTest(body=body):
                self.upstream.body = body
                self.assertEqual(self.call(body=form()), (502, {"error": {"message": "Invalid upstream transcription response"}}))

    def test_upstream_status_preserved_without_echoing_body_or_reason(self):
        output = io.StringIO()
        with redirect_stderr(output):
            for status in (400, 401, 403, 429, 500, 503, 599):
                with self.subTest(status=status):
                    self.upstream.status = status
                    self.upstream.reason = "Bearer secret-key"
                    self.upstream.body = b'{"error":"Bearer secret-key"}'
                    actual_status, value = self.call(body=form(), headers={"Authorization": "Bearer secret-key"})
                    self.assertEqual(actual_status, status)
                    self.assertIn(str(status), value["error"]["message"])
                    self.assertNotIn("secret-key", json.dumps(value))
        self.assertNotIn("secret-key", output.getvalue())

    def test_redirects_are_returned_without_forwarding_credentials(self):
        self.upstream.status = 307
        self.upstream.location = f"http://127.0.0.1:{self.upstream.server_port}/credential-target"
        self.assertEqual(self.call(body=form(), headers={"Authorization": "Bearer secret"})[0], 307)
        self.assertEqual(len(self.upstream.requests), 1)
        self.assertEqual(self.upstream.requests[0][0], "/v1/asr/transcribe")

    def test_connection_failure_and_health_without_upstream(self):
        self.adapter.base_url = "http://127.0.0.1:0/v1"
        self.assertEqual(self.call(method="GET", path="/health"), (200, {"status": "ok"}))
        self.assertEqual(self.call(body=form()), (502, {"error": {"message": "Could not reach upstream transcription service"}}))
        self.assertEqual(self.upstream.requests, [])

    def test_invalid_incoming_requests_are_json_errors_without_forwarding(self):
        duplicate = form()[:-10] + form()
        for body, headers, status in [
            (b"invalid", {}, 400),
            (form(), {"Content-Type": "application/json"}, 400),
            (form(), {"Content-Type": "multipart/form-data"}, 400),
            (form(audio=None), {}, 400),
            (form(audio=b""), {}, 400),
            (form()[:-8], {}, 400),
            (duplicate, {}, 400),
            (form(language=b"\xff"), {}, 400),
            (form().replace(b'name="file"', b'name="unknown"'), {}, 400),
            (b"", {"Content-Length": "invalid"}, 400),
            (b"", {"Content-Length": "-1"}, 411),
            (b"", {"Content-Length": str(MAX_UPLOAD + 1)}, 413),
        ]:
            with self.subTest(body=body, headers=headers):
                actual_status, value = self.call(body=body, headers=headers)
                self.assertEqual(actual_status, status)
                self.assertIsInstance(value["error"]["message"], str)
        self.assertEqual(self.upstream.requests, [])
        self.assertEqual(self.call(method="GET", path="/unknown")[0], 404)
        self.assertEqual(self.call(path="/unknown")[0], 404)
        self.assertEqual(self.call(method="PUT", body=b"")[0], 501)

    def test_four_requests_can_reach_upstream_concurrently(self):
        self.upstream.barrier = threading.Barrier(4)
        with ThreadPoolExecutor(max_workers=4) as executor:
            results = list(executor.map(lambda _: self.call(body=form()), range(4)))
        self.assertEqual(results, [(200, {"text": "Recognized"})] * 4)
        self.assertEqual(len(self.upstream.requests), 4)

    def test_multipart_parser_preserves_fields(self):
        self.assertEqual(multipart('multipart/form-data; boundary="test"', form(model="model", language="ko")), {"model": "model", "language": "ko", "file": b"RIFF\0\xffWAVE"})


if __name__ == "__main__":
    unittest.main()
