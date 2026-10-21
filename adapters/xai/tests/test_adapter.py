from concurrent.futures import ThreadPoolExecutor
from email import policy
from email.parser import BytesParser
import http.client
import http.server
import json
from pathlib import Path
import sys
import threading
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))
from notepipe_asr_xai import Handler, MAX_UPLOAD, multipart


def form(audio, model=None, language=None):
    parts = []
    for name, value in [("model", model), ("language", language), ("file", audio)]:
        if value is None:
            continue
        value = value if isinstance(value, bytes) else value.encode()
        parts.append(f'--test\r\nContent-Disposition: form-data; name="{name}"\r\n\r\n'.encode() + value + b"\r\n")
    return b"".join(parts) + b"--test--\r\n"


class Upstream(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        self.server.requests.append((self.path, self.headers, body))
        if self.server.barrier is not None:
            self.server.barrier.wait(timeout=5)
        self.send_response(self.server.status, "untrusted-secret")
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(self.server.body)))
        if self.server.location:
            self.send_header("Location", self.server.location)
        self.end_headers()
        self.wfile.write(self.server.body)

    def do_GET(self):
        self.server.requests.append((self.path, self.headers, b""))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"text":"redirected"}')

    def log_message(self, *args):
        pass


class AdapterTests(unittest.TestCase):
    def start_server(self, handler):
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.01})
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(thread.join)
        self.addCleanup(server.shutdown)
        return server

    def setUp(self):
        self.upstream = self.start_server(Upstream)
        self.upstream.requests = []
        self.upstream.status = 200
        self.upstream.body = json.dumps({"text": "안녕하세요 — hello", "words": []}).encode()
        self.upstream.location = None
        self.upstream.barrier = None
        self.adapter = self.start_server(Handler)
        self.adapter.base_url = f"http://127.0.0.1:{self.upstream.server_port}/custom/v1/"

    def call(self, method="POST", path="/v1/audio/transcriptions", body=None, headers=None):
        connection = http.client.HTTPConnection(*self.adapter.server_address, timeout=5)
        try:
            connection.request(method, path, body, {"Content-Type": "multipart/form-data; boundary=test", **(headers or {})})
            response = connection.getresponse()
            self.assertEqual(response.getheader("Content-Type"), "application/json")
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def test_health_is_immediate_and_does_not_contact_upstream(self):
        self.assertEqual(self.call("GET", "/health"), (200, {"status": "ok"}))
        self.assertEqual(self.upstream.requests, [])

    def test_forwarded_shape_binary_order_and_authorization(self):
        audio = b"RIFF\0\xff\r\n--test-not-a-boundary\r\nWAVE"
        result = self.call(body=form(audio, "grok-voice-transcribe-2.0", "ko"), headers={"Authorization": "Custom untrusted-secret"})
        self.assertEqual(result, (200, {"text": "안녕하세요 — hello"}))
        path, headers, body = self.upstream.requests[0]
        self.assertEqual(path, "/custom/v1/stt")
        self.assertEqual(headers["Authorization"], "Custom untrusted-secret")
        message = BytesParser(policy=policy.default).parsebytes(f'Content-Type: {headers["Content-Type"]}\r\n\r\n'.encode() + body)
        parts = list(message.iter_parts())
        self.assertEqual([p.get_param("name", header="content-disposition") for p in parts], ["model", "language", "file"])
        self.assertEqual([p.get_payload(decode=True) for p in parts], [b"grok-voice-transcribe-2.0", b"ko", audio])
        self.assertEqual(parts[-1].get_content_type(), "audio/wav")
        self.assertEqual(parts[-1].get_filename(), "audio.wav")

    def test_optional_fields_and_authorization_are_omitted_when_unset(self):
        for model, language in [(None, None), ("", ""), ("model", None), (None, "ja")]:
            with self.subTest(model=model, language=language):
                self.assertEqual(self.call(body=form(b"audio", model, language))[0], 200)
                _, headers, body = self.upstream.requests[-1]
                self.assertIsNone(headers.get("Authorization"))
                expected = {"file": b"audio"}
                if model:
                    expected["model"] = model
                if language:
                    expected["language"] = language
                self.assertEqual(multipart(headers["Content-Type"], body), expected)

    def test_empty_transcript_is_valid(self):
        self.upstream.body = b'{"text":""}'
        self.assertEqual(self.call(body=form(b"audio")), (200, {"text": ""}))

    def test_four_requests_reach_upstream_concurrently(self):
        self.upstream.barrier = threading.Barrier(4)
        with ThreadPoolExecutor(max_workers=4) as executor:
            results = list(executor.map(lambda _: self.call(body=form(b"audio")), range(4)))
        self.assertEqual(results, [(200, {"text": "안녕하세요 — hello"})] * 4)
        self.assertEqual(len(self.upstream.requests), 4)

    def test_upstream_status_is_preserved_without_echoing_body_or_reason(self):
        for status in [400, 401, 403, 413, 429, 500, 503, 599]:
            with self.subTest(status=status):
                self.upstream.status = status
                self.upstream.body = b'{"error":{"message":"untrusted-secret"}}'
                code, value = self.call(body=form(b"audio"), headers={"Authorization": "Bearer untrusted-secret"})
                self.assertEqual(code, status)
                self.assertIn(str(status), value["error"]["message"])
                self.assertNotIn("untrusted-secret", json.dumps(value))

    def test_redirect_does_not_forward_credentials_to_another_endpoint(self):
        self.upstream.status = 302
        self.upstream.location = f"http://127.0.0.1:{self.upstream.server_port}/leak"
        code, value = self.call(body=form(b"audio"), headers={"Authorization": "Bearer untrusted-secret"})
        self.assertEqual(code, 302)
        self.assertNotIn("untrusted-secret", json.dumps(value))
        self.assertEqual(len(self.upstream.requests), 1)

    def test_invalid_upstream_json_or_transcript_is_a_gateway_error(self):
        for body in [b"not JSON: untrusted-secret", b"\xff", b"null", b"[]", b"{}", b'{"text":null}', b'{"text":42}', br'{"text":"\ud800"}', b'{"text":"ignored","error":{"message":"untrusted-secret"}}']:
            with self.subTest(body=body):
                self.upstream.body = body
                code, value = self.call(body=form(b"audio"))
                self.assertEqual(code, 502)
                self.assertNotIn("untrusted-secret", json.dumps(value))

    def test_connection_failure_is_a_json_gateway_error(self):
        self.upstream.shutdown()
        self.upstream.server_close()
        code, value = self.call(body=form(b"audio"))
        self.assertEqual(code, 502)
        self.assertIsInstance(value["error"]["message"], str)

    def test_malformed_uploads_and_unknown_routes_are_json_errors(self):
        for method, path, body, headers, status in [
            ("GET", "/unknown", None, {}, 404),
            ("POST", "/unknown", b"", {}, 404),
            ("PUT", "/v1/audio/transcriptions", b"", {}, 501),
            ("POST", "/v1/audio/transcriptions", b"invalid", {}, 400),
            ("POST", "/v1/audio/transcriptions", form(None, "model"), {}, 400),
            ("POST", "/v1/audio/transcriptions", form(b"audio")[:-8], {}, 400),
            ("POST", "/v1/audio/transcriptions", form(b"audio"), {"Content-Type": "application/json"}, 400),
            ("POST", "/v1/audio/transcriptions", form(b"audio"), {"Content-Type": "multipart/form-data"}, 400),
            ("POST", "/v1/audio/transcriptions", form(b"audio", b"\xff"), {}, 400),
            ("POST", "/v1/audio/transcriptions", b"", {"Content-Length": "invalid"}, 400),
            ("POST", "/v1/audio/transcriptions", b"", {"Content-Length": "-1"}, 411),
            ("POST", "/v1/audio/transcriptions", b"", {"Content-Length": str(MAX_UPLOAD + 1)}, 413),
        ]:
            with self.subTest(method=method, path=path, body=body, headers=headers):
                code, value = self.call(method, path, body, headers)
                self.assertEqual(code, status)
                self.assertIsInstance(value["error"]["message"], str)
        self.assertEqual(self.upstream.requests, [])

    def test_multipart_rejects_duplicate_fields(self):
        body = form(b"a")[:-10] + form(b"b")
        self.assertEqual(self.call(body=body)[0], 400)
        self.assertEqual(self.upstream.requests, [])


if __name__ == "__main__":
    unittest.main()
