#!/usr/bin/python3
"""Unmount one external USB volume with OLC stopped; never force an unmount."""
import json
import os
from pathlib import Path
import subprocess
import sys
import urllib.request

RESULT = Path('/run/user') / str(os.getuid()) / 'olc-usb-result.json'


def volumes():
    tree = json.loads(subprocess.check_output(
        ['/usr/bin/lsblk', '--json', '--paths', '-o', 'NAME,TRAN,LABEL,MOUNTPOINTS'], text=True))
    found = []

    def nodes(node):
        yield node
        for child in node.get('children', []):
            yield from nodes(child)

    for disk in tree['blockdevices']:
        if disk.get('tran') != 'usb':
            continue
        mounted = [n for n in nodes(disk) if any(n.get('mountpoints') or [])]
        mounts = [p for n in mounted for p in n['mountpoints'] if p]
        # Reject the entire device if any partition has a system/unknown mount.
        if not mounts or not all(p.startswith(('/media/', '/run/media/')) for p in mounts):
            continue
        found.append({'device': disk['name'],
                      'label': ', '.join(n.get('label') or n['name'] for n in mounted),
                      'blocks': [n['name'] for n in mounted], 'mounts': mounts})
    return found


def report(job, state, message):
    temporary = RESULT.with_suffix('.tmp')
    temporary.write_text(json.dumps({'job': job, 'state': state, 'message': message}))
    temporary.replace(RESULT)


def unmount(device, url, job):
    stopped = False
    report(job, 'running', 'Unmounting USB…')
    try:
        volume = next((v for v in volumes() if v['device'] == device), None)
        if volume is None:
            raise RuntimeError('USB is no longer mounted or is not an external USB volume.')
        with urllib.request.urlopen(url, timeout=5) as response:
            live = json.load(response)
        # Discovery loss cannot prove a previously loaded track has finished;
        # require the operator to turn off/disconnect players before ejecting.
        if live.get('decks') or live.get('directPeers'):
            raise RuntimeError('Turn off or disconnect the CDJs before unmounting the USB.')
        stopped = True
        subprocess.run(['/usr/bin/systemctl', '--user', 'stop', 'olc-host.service'], check=True, timeout=30)
        for block in volume['blocks']:
            result = subprocess.run(['/usr/bin/sudo', '-n', '/usr/bin/udisksctl', 'unmount', '--no-user-interaction', '-b', block], capture_output=True, text=True, timeout=30)
            if result.returncode:
                raise RuntimeError(result.stderr.strip() or 'Unmount failed. Keep the USB connected.')
        if any(v['device'] == device for v in volumes()):
            raise RuntimeError('USB is still mounted. Keep it connected.')
        report(job, 'done', 'USB unmounted. You can safely remove it.')
    except Exception as error:
        report(job, 'error', str(error))
    finally:
        if stopped:
            result = subprocess.run(['/usr/bin/systemctl', '--user', 'start', 'olc-host.service'], capture_output=True, text=True, timeout=30)
            if result.returncode:
                report(job, 'error', 'OLC could not restart. Check the USB mount and restart OLC over SSH.')


if __name__ == '__main__':
    if sys.argv[1] == 'list':
        result = json.loads(RESULT.read_text()) if RESULT.exists() else None
        print(json.dumps({'volumes': volumes(), 'result': result}))
    elif sys.argv[1] == 'unmount' and len(sys.argv) == 5:
        unmount(*sys.argv[2:])
    else:
        raise SystemExit('Invalid USB helper action')
