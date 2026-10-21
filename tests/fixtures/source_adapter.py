"""Offline source adapter fixture; only the successful CLI case needs ffmpeg."""
import argparse
import json
import os
import signal
from pathlib import Path
import subprocess
import sys
import time

# Like the real adapter, configured arguments precede URL --out DIR.
url, option, directory = sys.argv[-3:]
assert option == '--out'
arguments = sys.argv[1:-3]
separator = arguments.index('--') if '--' in arguments else len(arguments)
parser = argparse.ArgumentParser()
parser.add_argument('--record', type=Path, required=True)
parser.add_argument('--behavior', default='healthy')
args = parser.parse_args(arguments[:separator])
args.record.mkdir(parents=True, exist_ok=True)
(args.record / 'pid').write_text(str(os.getpid()))
with (args.record / 'calls').open('a') as log:
    log.write(json.dumps({'url': url, 'extra': arguments[separator + 1:]}) + '\n')
print('source adapter fixture diagnostics', file=sys.stderr, flush=True)
out = Path(directory)
assert not any(out.iterdir())
if args.behavior == 'hang':
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    code = "import signal,time; from pathlib import Path; signal.signal(signal.SIGTERM, lambda *_: (Path(%r).write_text('stopped'), exit(0))); Path(%r).write_text('ready'); time.sleep(60)" % (str(args.record / 'child-stopped'), str(args.record / 'child-ready'))
    subprocess.Popen([sys.executable, '-c', code])
    time.sleep(60)
if args.behavior == 'exit':
    sys.exit(7)
if args.behavior == 'malformed':
    print('{invalid', flush=True)
    sys.exit(0)
if args.behavior in {'parent', 'absolute'}:
    print(json.dumps({'media': '../outside.mp4' if args.behavior == 'parent' else str(args.record / 'outside.mp4')}), flush=True)
    sys.exit(0)
if args.behavior == 'oversized':
    print('x' * (1024 * 1024 + 1), flush=True)
    sys.exit(0)
print(json.dumps({'progress': 0.25}), flush=True)
media = out / 'media.mp4'
if args.behavior in {'files', 'orphan', 'fragmented'}:
    media.write_bytes(b'cached media')
else:
    command = ['ffmpeg', '-nostdin', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'color=c=blue:s=64x32:d=3']
    if args.behavior == 'speech':
        command += ['-i', str(Path(__file__).with_name('speech.wav')), '-shortest']
    subprocess.run(command + ['-c:v', 'mpeg4', str(media)], check=True)
tracks = []
for language, automatic, text in [('fr', False, 'French captions.'), ('en', True, 'Automatic captions.'), ('en', False, 'Welcome to the source adapter.')]:
    name = f'caption-{language}-{automatic}.srt'
    (out / name).write_text(f'1\n00:00:00,000 --> 00:00:02,500\n{text}\n', encoding='utf-8')
    tracks.append({'path': name, 'language': language, 'automatic': automatic})
print(json.dumps({'progress': 1.0}), flush=True)
result = json.dumps({'media': media.name, 'title': 'Fixture source', 'author': 'Fixture author', 'duration': 3, 'language': 'en', 'url': url, 'subtitles': tracks})
if args.behavior == 'orphan':
    child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])
    (args.record / 'child-pid').write_text(str(child.pid))
if args.behavior == 'fragmented':
    sys.stdout.write(result[:20])
    sys.stdout.flush()
    time.sleep(0.15)
    result = result[20:]
print(result, flush=True)
