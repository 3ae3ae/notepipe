"""Offline llama-server stand-in, controlled by files in --record."""
import argparse
import http.server
import json
import os
from pathlib import Path
import signal
import sys

parser = argparse.ArgumentParser(add_help=False, allow_abbrev=False)
parser.add_argument("--host", required=True)
parser.add_argument("--port", type=int, required=True)
parser.add_argument("--record", type=Path, required=True)
args, unknown = parser.parse_known_args()
record = args.record
record.joinpath("args.json").write_text(json.dumps(sys.argv[1:]))


def stop(*_):
    if not record.joinpath("ignore-term").exists():
        record.joinpath("terminated").touch()
        raise SystemExit(0)


signal.signal(signal.SIGTERM, stop)


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if record.joinpath("ready").exists() else 503)
        self.end_headers()

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        record.joinpath("request.json").write_text(json.dumps({
            "path": self.path, "body": body, "headers": dict(self.headers),
        }))
        with record.joinpath("requests.jsonl").open("a") as requests:
            requests.write(json.dumps(body) + "\n")
        config = record / "response.json"
        config = json.loads(config.read_text()) if config.exists() else {}
        # A list answers successive requests in order and then repeats its last entry.
        if isinstance(config, list):
            count = len(record.joinpath("requests.jsonl").read_text().splitlines())
            config = config[min(count, len(config)) - 1]
        response = config.get("body", json.dumps({"choices": [{"message": {"content": "Recognized."}}]})).encode()
        self.send_response(config.get("status", 200), "untrusted-secret")
        if "location" in config:
            self.send_header("Location", config["location"])
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, *_):
        pass


with http.server.HTTPServer((args.host, args.port), Handler) as server:
    server.timeout = 0.05
    record.joinpath("pid").write_text(str(os.getpid()))
    while True:
        if record.joinpath("exit").exists():
            for number in range(30):
                print(f"child diagnostic {number}", file=sys.stderr, flush=True)
            raise SystemExit(int(record.joinpath("exit").read_text()))
        server.handle_request()
