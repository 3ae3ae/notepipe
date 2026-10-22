"""Offline file-stage fixture; all paths are supplied as arguments."""
import json
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
