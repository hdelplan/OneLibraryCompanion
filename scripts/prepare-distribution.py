#!/usr/bin/env python3
"""Make an allowlisted source snapshot for review before a GitHub publication."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parent.parent
FILES = ['Cargo.toml', 'Cargo.lock', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'README.md', '.gitignore', 'AGENTS.md']
TREES = ['source/core', 'source/host', 'source/ui', 'source/desktop', 'vendor/prolink', 'third-party-licenses', 'packaging', '.github']
SCRIPTS = ['check-node.cjs', 'bootstrap-mac.sh', 'build-linux.sh', 'build-macos.sh', 'build-pi-cross.sh', 'check-app.sh', 'check-distribution.py', 'check-docs.py', 'collect-third-party-notices.py', 'package-linux.py', 'package-mac-networking.sh', 'position_signals.py', 'prepare-distribution.py', 'setup.sh', 'smoke-host.py', 'test_position_signals.py', 'test-mac-networking.py', 'tests/test_usb_unmount.py', 'tests/test_publication.py', 'trace-bar-position.py']
DOCS = ['distribution.md', 'release-notes.md', 'architecture.md', 'configuration.md', 'set-history.md', 'development.md', 'hardware-testing.md', 'compatibility.md', 'local-usb.md', 'screenshots.md', 'cdj-configurations.md']
PUBLIC_AGENTS = '''# Project requirements

- Keep UI and feature improvements shared across Mac and Raspberry Pi. Implement shared behavior in source/ui and source/host.
- Keep the UI in English. Preserve existing workspace changes.
- Consult the user before material changes to CDJ-facing source identity, metadata selection or serving behavior.
- Keep distribution documentation focused on supported Mac and Raspberry Pi functionality and current behavior. Document only implemented features and known limitations; exclude future plans, proposed features and roadmaps.
'''
EXCLUDED = {'.git', 'target', 'builds', 'dist', 'node_modules', 'DerivedData', 'xcuserdata', 'com.apple.DeveloperTools', '__pycache__', '.DS_Store', '.local', '.venv', '.pytest_cache'}
BANNED = {'.p12', '.mobileprovision', '.cer', '.pem', '.key', '.db', '.pdb', '.wav', '.mp3', '.aiff', '.flac', '.log', '.pyc', '.xcuserstate'}
# Original synthetic regression signals, never user music. Pin their bytes so
# an unrelated audio file cannot silently enter the source distribution.
FIXTURES = {
    'source/host/tests/fixtures/transcoding/reference.flac': 'e24560f13fc9ab2af5440550ec8ef0c323aa78d81a7439af3a2c47fa59e7ed57',
    'source/host/tests/fixtures/transcoding/reference.m4a': 'e50b13e40bafaf8b0b57bdc873e51e15f65d544201653038dd4d7f418b6914c3',
    'source/host/tests/fixtures/transcoding/reference.wav': 'baa2705b98262623dea90b3360335448ff177972b5e3c5aa766b4d4c118c08ef',
}


def source_files(directory):
    # Prune generated/private directories before walking them, not after reading
    # an entire node_modules, Cargo target or local-work tree into candidates.
    for parent, directories, files in os.walk(directory):
        directories[:] = [name for name in directories if name not in EXCLUDED and not name.startswith('dist-')]
        for name in files:
            yield Path(parent) / name


def prepare(destination):
    destination = destination.resolve()
    if destination.exists():
        raise SystemExit('Choose a new empty destination; existing snapshots are never overwritten.')
    candidates = [ROOT / name for name in FILES]
    candidates.extend(ROOT / 'scripts' / name for name in SCRIPTS)
    for tree in TREES:
        candidates.extend(source_files(ROOT / tree))
    candidates.extend(ROOT / 'docs' / name for name in DOCS)
    candidates.extend(path for path in (ROOT / 'docs/screenshots').rglob('*') if path.is_file())
    manifest = []
    for source in sorted(set(candidates)):
        relative = source.relative_to(ROOT)
        if any(part in EXCLUDED or part.startswith('dist-') for part in relative.parts):
            continue
        fixture = FIXTURES.get(relative.as_posix())
        if fixture and hashlib.sha256(source.read_bytes()).hexdigest() != fixture:
            raise SystemExit(f'Synthetic fixture changed; review before publication: {relative}')
        if source.is_symlink() or (source.suffix.lower() in BANNED and not fixture) or source.name.startswith('.env'):
            raise SystemExit(f'Review excluded/sensitive file before publication: {relative}')
        if source.stat().st_size > 2 * 1024 * 1024:
            raise SystemExit(f'Review unusually large source file: {relative}')
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if relative.as_posix() == 'AGENTS.md':
            target.write_text(PUBLIC_AGENTS)
        else:
            shutil.copy2(source, target)
        manifest.append(f'{hashlib.sha256(target.read_bytes()).hexdigest()}  {relative.as_posix()}')
    (destination / 'SOURCE-MANIFEST.sha256').write_text('\n'.join(manifest) + '\n')
    print(f'Prepared {len(manifest)} source files at {destination}')
    print('Review the manifest and repository visibility before publishing.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    prepare(parser.parse_args().destination)
