import argparse
import http.server
import json
import os
from pathlib import Path
import signal
import sys

parser = argparse.ArgumentParser()
parser.add_argument('--host', required=True)
parser.add_argument('--port', required=True, type=int)
parser.add_argument('--record', required=True, type=Path)
args = parser.parse_args()
args.record.mkdir(exist_ok=True)


def stop(signum, frame):
    (args.record / 'terminated').write_text('SIGTERM')
    sys.exit(0)


signal.signal(signal.SIGTERM, stop)
(args.record / 'pid').write_text(str(os.getpid()))
(args.record / 'port').write_text(str(args.port))


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *unused):
        pass

    def do_GET(self):
        assert self.path == '/health'
        self.send_response(200)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def do_POST(self):
        assert self.path == '/v1/chat/completions'
        body = self.rfile.read(int(self.headers['Content-Length']))
        with (args.record / 'calls').open('a') as log:
            log.write(json.dumps(json.loads(body)) + '\n')
        with (args.record / 'authorization').open('a') as log:
            log.write(self.headers.get('Authorization', '') + '\n')
        response = b'{"choices":[{"message":{"content":"Adapter summary."}}],"usage":{"prompt_tokens":3,"completion_tokens":2}}'
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(response)))
        self.end_headers()
        self.wfile.write(response)


http.server.ThreadingHTTPServer((args.host, args.port), Handler).serve_forever()
