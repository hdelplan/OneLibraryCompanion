#!/usr/bin/env python3
"""Check desktop build assets and a running, isolated desktop host without CDJ traffic."""
import argparse
import json
from pathlib import Path
import urllib.request
import urllib.error

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--url', help='Optional isolated desktop host URL for read-only HTTP checks')
args = parser.parse_args()
root = Path(__file__).resolve().parent.parent
prod = root / 'source/ui/dist'
for folder in (prod,):
    assert (folder / 'index.html').is_file(), f'Build {folder} first'
    assert 'OneLibraryCompanion' in (folder / 'index.html').read_text()
    for source in [root / 'LICENSE', root / 'THIRD_PARTY_NOTICES.md', *sorted((root / 'third-party-licenses').iterdir())]:
        bundled = folder / 'licenses' / source.relative_to(root)
        assert bundled.is_file() and bundled.read_bytes() == source.read_bytes(), f'Missing or stale bundled notice: {bundled}'
text = '\n'.join(p.read_text() for p in prod.rglob('*.js'))
for marker in ['DIAGNOSTICS & EXPERIMENTS', '/api/diagnostics/cue-window', '/api/diagnostics/native-cues', '/diagnostics/local-usb', 'Choose an offline preview in TEST', 'PIONEERCOMPANION / SET HISTORY']:
    assert marker not in text, f'Production asset leaked: {marker}'
assert not list(prod.glob('assets/Experiments-*.js')), 'Production included the experiments chunk'
if args.url:
    def request(path):
        with urllib.request.urlopen(args.url.rstrip('/') + path, timeout=3) as response:
            return json.load(response)
    health = request('/api/health')
    assert health['product'] == 'OneLibraryCompanion' and health['experiments'] is False
    assert health['live'] is False, 'Use an isolated offline smoke-test host'
    info = request('/api/app')
    assert info['authentication'] == 'none'
    for path in ['/diagnostics/local-usb', '/diagnostics/direct-ip', '/api/diagnostics/local-usb', '/api/diagnostics/cue-window', '/api/diagnostics/cue-window/reports', '/api/diagnostics/native-cues', '/api/diagnostics/jog', '/api/diagnostics/jog/state', '/api/diagnostics/direct-ip', '/api/diagnostics/direct-status']:
        for method in ['GET', 'POST']:
            try:
                with urllib.request.urlopen(urllib.request.Request(args.url + path, method=method), timeout=3):
                    raise AssertionError(f'Diagnostic route exposed: {method} {path}')
            except urllib.error.HTTPError as error:
                assert error.code in (404, 405), (path, error.code)
    assert isinstance(request('/api/library/sources'), dict)
    assert isinstance(request('/api/sets'), dict)
print('Distribution checks passed: production excludes TEST' + ('; desktop APIs verified' if args.url else ''))
