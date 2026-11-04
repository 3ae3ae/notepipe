"""Offline file-stage fixture; all paths are supplied as arguments."""
import json
import shutil
import subprocess
from pathlib import Path
import sys
import time

print('run fixture diagnostics', file=sys.stderr, flush=True)
mode = sys.argv[1]
if mode == 'exit':
    sys.exit(7)
if mode == 'missing':
    sys.exit(0)
if mode == 'text':
    sys.stdout.buffer.write(sys.argv[2].encode())
elif mode == 'files':
    paths = [Path(p) for p in sys.argv[2:]]
    assert all(p.is_absolute() for p in paths)
    paths[-1].write_bytes(b''.join(p.read_bytes() for p in paths[:-1]))
elif mode == 'media':
    source, output, variant, record = sys.argv[2:]
    source = Path(source)
    assert source.is_file()
    Path(record).write_text(str(source))
    if output == '-':
        sys.stdout.buffer.write(source.read_bytes())
    elif variant == 'copy':
        shutil.copyfile(source, output)
    else:
        subprocess.run(['ffmpeg', '-nostdin', '-loglevel', 'error', '-y', '-i', str(source), '-af', f'volume={variant}', output], check=True)
elif mode == 'images':
    output, variant = Path(sys.argv[2]), sys.argv[3]
    (output / 'nested').mkdir(parents=True)
    (output / 'nested/frame.png').write_bytes(bytes.fromhex('89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000b49444154789c636000020000050001a5f645400000000049454e44ae426082'))
    frames = [{'t': 0, 'image': 'nested/frame.png'}]
    if variant == 'unordered':
        frames = [{'t': 1, 'image': 'nested/frame.png'}, frames[0]]
    elif variant == 'negative':
        frames[0]['t'] = -1
    elif variant == 'infinite':
        frames[0]['t'] = float('inf')
    elif variant == 'escape':
        frames[0]['image'] = '../outside.png'
    elif variant == 'missing':
        frames[0]['image'] = 'missing.png'
    elif variant == 'directory':
        frames[0]['image'] = 'nested'
    (output / 'images.json').write_text('invalid' if variant == 'json' else json.dumps(frames))
elif mode == 'counter':
    counter = Path(sys.argv[2])
    counter.write_bytes((counter.read_bytes() if counter.exists() else b'') + b'run\n')
    sys.stdout.buffer.write(sys.stdin.buffer.read())
elif mode in {'document', 'outside', 'bad-time', 'schema'}:
    document = json.load(sys.stdin)
    document['extension'] = {'keep': [1, 2, 3]}
    document['meta']['extension'] = 'metadata'
    document['sections'][0]['extension'] = 'section'
    document['sections'][0]['speech'][0]['extension'] = 'paragraph'
    if mode == 'outside':
        document['sections'][0]['image'] = '../outside.jpg'
    if mode == 'bad-time':
        document['sections'][0]['speech'][0]['start'] = -1
    if mode == 'schema':
        document['schema'] = 2
    print(json.dumps(document))
elif mode == 'wait':
    Path(sys.argv[2]).write_text('ready')
    time.sleep(60)
else:
    assert mode == 'stdin'
    sys.stdout.buffer.write(sys.stdin.buffer.read())
