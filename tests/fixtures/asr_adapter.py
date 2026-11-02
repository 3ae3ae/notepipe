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
with (args.record / 'starts').open('a', newline='\n') as log:
    log.write('start\n')


def stop(signum, frame):
    (args.record / 'terminated').write_text('SIGTERM')
    sys.exit(0)


# Install the handler before announcing the pid, so tests that wait for the pid can rely on it.
signal.signal(signal.SIGTERM, stop)
(args.record / 'pid').write_text(str(os.getpid()))
print('adapter stdout', flush=True)
print('adapter stderr', file=sys.stderr, flush=True)
if args.behavior == 'exit':
    sys.exit(3)
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
if args.behavior in {'escaped', 'escaped-hang'}:
    child = subprocess.Popen([sys.executable, '-c', 'import signal;signal.pause()'], start_new_session=True)
    (args.record / 'escaped-pid').write_text(str(child.pid))
if args.behavior == 'stubborn':
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    code = "import time; from pathlib import Path; Path(%r).write_text('ready'); time.sleep(30)" % str(args.record / 'child-ready')
    child = subprocess.Popen([sys.executable, '-c', code])
    (args.record / 'child-pid').write_text(str(child.pid))
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
        if args.behavior == 'exit-request':
            os.write(2, b'invalid: \xff\nlast diagnostic\n')
            os._exit(7)
        if args.behavior == 'escaped-hang':
            time.sleep(30)
        if args.behavior == 'slow':
            (args.record / 'transcribing').write_text('ready')
            time.sleep(30)
            (args.record / 'responded').write_text('ready')
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
        elif args.behavior == 'output-limit':
            self.send_response(422)
            response = b'{"error":{"code":"output_limit","message":"x"}}'
        elif args.behavior == 'error' or (args.behavior == 'fail-after-first' and len((args.record / 'calls').read_text().splitlines()) > 1 and not (args.record / 'release').exists()):
            self.send_response(400)
            response = b'{}'
        else:
            self.send_response(200)
            content = 'Fixture summary.'
            if self.path == '/v1/chat/completions' and json.loads(body).get('response_format'):
                items = json.loads(json.loads(body)['messages'][1]['content'][0]['text'])
                content = json.dumps({'segments': [{'id': item['id'], 'text': item['text']} for item in items]})
            first = len((args.record / 'calls').read_text().splitlines()) == 1
            text = '' if args.behavior == 'empty' or (args.behavior == 'empty-first' and first) else 'Fixture transcript.'
            value = {'choices': [{'message': {'content': content}}]} if self.path == '/v1/chat/completions' else {'text': text}
            if args.behavior == 'warning':
                value['warning'] = '  Diagnostic\n\t' + '界' * 400
            if args.behavior == 'missing-text-first' and first:
                value = {'text': None}
            if args.behavior in ('warning', 'empty-first', 'missing-text-first', 'invalid-json-first'):
                value['usage'] = {'prompt_tokens': 3, 'completion_tokens': 2}
            response = b'not JSON' if args.behavior == 'invalid-json-first' and first else json.dumps(value).encode()
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(response)))
        self.end_headers()
        self.wfile.write(response)
        if args.behavior == 'exit-idle':
            def exit_idle():
                while not (args.record / 'between-consumers').exists():
                    time.sleep(0.01)
                os.write(2, b'last diagnostic\n')
                os._exit(7)
            threading.Thread(target=exit_idle, daemon=True).start()


server = http.server.ThreadingHTTPServer((args.host, args.port), Handler)
(args.record / 'ready').write_text('ready')
server.serve_forever()
