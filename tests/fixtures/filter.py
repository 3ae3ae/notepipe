"""Offline note filter fixture."""
import json
import sys

print('filter fixture diagnostics', file=sys.stderr)
note = json.load(sys.stdin)
assert note['schema'] == 1
assert isinstance(note['warnings'], list)
mode = sys.argv[1]
if mode == 'exit':
    sys.exit(7)
if mode == 'invalid':
    print('{invalid')
    sys.exit(0)
if mode == 'sections':
    note['sections'].append(note['sections'][0])
elif mode == 'fragments':
    note['sections'][0]['speech'].append(note['sections'][0]['speech'][0])
else:
    note['top'].append({'stage': 'filter', 'text': '## Filter top'})
    for section in note['sections']:
        section['image'] = '../ignored.jpg'
        for fragment in section['speech']:
            fragment['text'] = fragment['text'].upper()
            fragment.setdefault('below', []).append({'stage': 'filter', 'text': 'Filter below'})
            fragment['start'] = -999
            fragment['raw'] = 'ignored raw'
    note['meta']['title'] = 'ignored title'
    note['warnings'] = ['ignored warning']
print(json.dumps(note))
