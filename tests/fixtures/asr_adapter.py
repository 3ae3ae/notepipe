"""Offline test process; no models or third-party Python packages."""
import argparse
import http.server
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import threading

parser = argparse.ArgumentParser()
parser.add_argument('--host', required=True)
parser.add_argument('--port', required=True, type=int)
parser.add_argument('--record', required=True, type=Path)
parser.add_argument('--behavior', default='healthy')
args = parser.parse_args()
args.record.mkdir(exist_ok=True)
(args.record / 'pid').write_text(str(os.getpid()))
print('adapter stdout', flush=True)
print('adapter stderr', file=sys.stderr, flush=True)
if args.behavior == 'exit':
    sys.exit(3)


def stop(signum, frame):
    (args.record / 'terminated').write_text('SIGTERM')
    sys.exit(0)


signal.signal(signal.SIGTERM, stop)
if args.behavior == 'launcher':
    child = subprocess.Popen([
        sys.executable, '-u', __file__, '--host', args.host, '--port', str(args.port),
        '--record', str(args.record), '--behavior', 'orphan',
    ])
    (args.record / 'child-pid').write_text(str(child.pid))
    while not (args.record / 'release-launcher').exists():
        time.sleep(0.01)
    sys.exit(0)
if args.behavior == 'orphan':
    deadline = threading.Timer(30, lambda: os._exit(0))
    deadline.daemon = True
    deadline.start()
if args.behavior == 'stubborn':
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    code = "import signal,time; from pathlib import Path; signal.signal(signal.SIGTERM, lambda *_: (Path(%r).write_text('SIGTERM'), exit(0))); Path(%r).write_text('ready'); time.sleep(30)" % (str(args.record / 'child-terminated'), str(args.record / 'child-ready'))
    subprocess.Popen([sys.executable, '-c', code])
    while not (args.record / 'child-ready').exists():
        time.sleep(0.01)


class Handler(http.server.BaseHTTPRequestHandler):
    checks = 0
    active = 0
    max_active = 0
    lock = threading.Lock()

    def log_message(self, *unused):
        pass

    def do_GET(self):
        assert self.path == '/health'
        Handler.checks += 1
        (args.record / 'health-checks').write_text(str(Handler.checks))
        status = 503 if args.behavior == 'never' or Handler.checks == 1 else (204 if Handler.checks == 2 else 200)
        self.send_response(status)
        self.send_header('Content-Length', '0')
        self.end_headers()

    def do_POST(self):
        body = self.rfile.read(int(self.headers['Content-Length']))
        with (args.record / 'calls').open('a') as log:
            log.write('request\n')
        (args.record / 'request').write_bytes(body)
        (args.record / 'authorization').write_text(self.headers.get('Authorization', ''))
        (args.record / 'endpoint').write_text(self.path)
        if args.behavior == 'parallel':
            with Handler.lock:
                Handler.active += 1
                Handler.max_active = max(Handler.max_active, Handler.active)
                (args.record / 'max-active').write_text(str(Handler.max_active))
            time.sleep(0.2)
            with Handler.lock:
                Handler.active -= 1
        if args.behavior == 'hang' or (args.behavior == 'resume' and len((args.record / 'calls').read_text().splitlines()) > 1 and not (args.record / 'release').exists()):
            (args.record / 'transcribing').write_text('ready')
            time.sleep(30)
        elif args.behavior == 'error' or (args.behavior == 'fail-after-first' and len((args.record / 'calls').read_text().splitlines()) > 1 and not (args.record / 'release').exists()):
            self.send_response(400)
            response = b'{}'
        else:
            self.send_response(200)
            response = json.dumps({'text': 'Fixture transcript.'}).encode()
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(response)))
        self.end_headers()
        self.wfile.write(response)


server = http.server.ThreadingHTTPServer((args.host, args.port), Handler)
(args.record / 'ready').write_text('ready')
server.serve_forever()
