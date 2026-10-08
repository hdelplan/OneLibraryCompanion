#!/usr/bin/env python3
"""Isolated desktop release smoke test; does not connect to or control CDJs."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary', type=Path)
parser.add_argument('--ui', type=Path, default=Path('source/ui/dist'))
args = parser.parse_args()
with socket.socket() as sock:
    sock.bind(('127.0.0.1', 0))
    port = sock.getsockname()[1]
url = f'http://127.0.0.1:{port}'


def request(path, body=None):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url + path, data=data, headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=3) as response:
        return json.load(response)


with tempfile.TemporaryDirectory(prefix='olc-release-smoke-') as directory:
    env = {k: v for k, v in os.environ.items() if not k.startswith(('OLC_', 'PIONEER_COMPANION_'))}
    env.update(OLC_BIND=f'127.0.0.1:{port}', OLC_UI_ROOT=str(args.ui.resolve()), OLC_DATA=directory, OLC_INSTANCE='release-smoke')
    with open(Path(directory) / 'host.log', 'wb') as log:
        child = None
        def start():
            process = subprocess.Popen([str(args.binary.resolve())], env=env, stdout=log, stderr=log)
            try:
                for _ in range(100):
                    if process.poll() is not None:
                        raise AssertionError('Host exited before readiness')
                    try:
                        health = request('/api/health')
                        assert health['instance'] == 'release-smoke'
                        assert health['experiments'] is False and health['live'] is False
                        return process
                    except urllib.error.URLError:
                        time.sleep(0.05)
                raise AssertionError('Host startup timed out')
            except BaseException:
                process.terminate()
                process.wait(timeout=5)
                raise
        try:
            child = start()
            for path in ['/diagnostics/local-usb', '/diagnostics/direct-ip', '/api/diagnostics/local-usb', '/api/diagnostics/cue-window', '/api/diagnostics/cue-window/reports', '/api/diagnostics/native-cues', '/api/diagnostics/jog', '/api/diagnostics/jog/state', '/api/diagnostics/direct-ip', '/api/diagnostics/direct-status']:
                for method in ['GET', 'POST']:
                    try:
                        urllib.request.urlopen(urllib.request.Request(url + path, method=method), timeout=3)
                        raise AssertionError(f'Exposed diagnostic endpoint: {method} {path}')
                    except urllib.error.HTTPError as error:
                        assert error.code in (404, 405), (path, error.code)
            assert request('/api/app')['authentication'] == 'none'
            assert isinstance(request('/api/library/sources'), dict)
            saved = request('/api/sets', {'action': 'start'})
            identifier = saved['activeId']
            request('/api/sets', {'action': 'metadata', 'id': identifier, 'title': 'OLC release smoke'})
            request('/api/sets', {'action': 'finish', 'id': identifier})
            assert request('/api/sets')['sets'][0]['title'] == 'OLC release smoke'
            assert request('/api/app', {'interface': None})['restartRequired'] is True
            # A second instance must exit, rather than sharing a port/CDJ session.
            duplicate = subprocess.Popen([str(args.binary.resolve())], env=env, stdout=log, stderr=log)
            assert duplicate.wait(timeout=5) != 0
            # A connected SSE browser must not prevent graceful Quit.
            stream = urllib.request.urlopen(url + '/api/live/events', timeout=3)
            assert stream.readline().startswith(b'data:')
            child.terminate()
            assert child.wait(timeout=5) == 0
            stream.close()
            child = start()
            assert request('/api/sets')['sets'][0]['title'] == 'OLC release smoke'
            assert request('/api/app')['interface'] is None
            child.terminate()
            assert child.wait(timeout=5) == 0
        finally:
            if child and child.poll() is None:
                child.kill()
                child.wait()
print('Release smoke passed: production routes, shared history, persistence, duplicate rejection and graceful SSE shutdown.')
