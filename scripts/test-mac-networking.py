#!/usr/bin/env python3
"""Verify live-process authorization without installing a privileged service."""
from pathlib import Path
import os, re, subprocess, tempfile, time
root = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='olc-auth-', dir='/tmp') as directory:
    work = Path(directory)
    source = root / 'source/desktop/macos/networking/tests'
    for name in ['server', 'client']:
        subprocess.run(['xcrun','clang','-Wall','-Wextra','-Werror','-framework','Security','-framework','CoreFoundation',str(source / f'auth-{name}.c'),'-o',str(work / name)],check=True)
    subprocess.run(['codesign','--force','--options','runtime','--sign','-',str(work/'client')],check=True)
    result = subprocess.run(['codesign','-d','--verbose=4',str(work/'client')],capture_output=True,text=True,check=True)
    codehash = re.search(r'^CDHash=([0-9a-f]{40})$', result.stderr,re.M)[1]
    policy = work/'hash'
    def run(contents, mode, expected):
        policy.write_text(contents); policy.chmod(mode)
        endpoint = work/'socket'
        server = subprocess.Popen([str(work/'server'),str(endpoint),str(policy)])
        try:
            deadline=time.monotonic()+3
            while not endpoint.exists():
                if server.poll() is not None or time.monotonic()>deadline: raise AssertionError('Broker did not start')
                time.sleep(.02)
            result=subprocess.run([str(work/'client'),str(endpoint)],timeout=5)
            assert result.returncode == expected, (contents,mode,result.returncode,expected)
            assert server.wait(timeout=5)==0
        finally:
            if server.poll() is None: server.kill(); server.wait()
            endpoint.unlink(missing_ok=True)
    run(codehash+'\n',0o644,0)
    run('0'*40+'\n',0o644,1)
    run(codehash+'\n',0o666,1)
    run('invalid\n',0o644,1)
    print('Helper authorization passed: signed client accepted; wrong identity, writable policy and malformed policy rejected.')
