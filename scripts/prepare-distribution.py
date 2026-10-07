#!/usr/bin/env python3
"""Make an allowlisted source snapshot for review before a private GitHub push."""
import argparse
import hashlib
from pathlib import Path
import shutil

ROOT = Path(__file__).resolve().parent.parent
FILES = ['Cargo.toml', 'Cargo.lock', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'README.md', '.gitignore', 'AGENTS.md']
TREES = ['source', 'vendor/prolink', 'third-party-licenses', 'packaging', 'scripts', '.github']
DOCS = ['distribution.md', 'release-notes.md', 'architecture.md', 'configuration.md', 'set-history.md', 'development.md', 'hardware-testing.md', 'compatibility.md', 'local-usb.md', 'screenshots.md']
OMITTED = {'source/ipad/README.md', 'vendor/prolink/Cargo.lock'}
PUBLIC_AGENTS = '''# Project requirements

- Keep UI and feature improvements shared across Mac and Raspberry Pi. Implement shared behavior in source/ui and source/host.
- Keep the UI in English. Preserve existing workspace changes.
- Keep distribution documentation focused on supported Mac and Raspberry Pi functionality and current behavior.
'''
EXCLUDED = {'.git', 'target', 'builds', 'dist', 'dist-ipad', 'node_modules', 'DerivedData', 'xcuserdata', 'com.apple.DeveloperTools', '__pycache__', '.DS_Store'}
BANNED = {'.p12', '.mobileprovision', '.cer', '.pem', '.key', '.db', '.pdb', '.wav', '.mp3', '.aiff', '.flac', '.log', '.pyc', '.xcuserstate'}


def prepare(destination):
    destination = destination.resolve()
    if destination.exists():
        raise SystemExit('Choose a new empty destination; existing snapshots are never overwritten.')
    candidates = [ROOT / name for name in FILES]
    for tree in TREES:
        candidates.extend(path for path in (ROOT / tree).rglob('*') if path.is_file())
    candidates.extend(ROOT / 'docs' / name for name in DOCS)
    candidates.extend(path for path in (ROOT / 'docs/screenshots').rglob('*') if path.is_file())
    manifest = []
    for source in sorted(set(candidates)):
        relative = source.relative_to(ROOT)
        if relative.as_posix() in OMITTED or any(part in EXCLUDED for part in relative.parts):
            continue
        if source.is_symlink() or source.suffix.lower() in BANNED or source.name.startswith('.env'):
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
    print('Review the manifest and private repository visibility before pushing.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    prepare(parser.parse_args().destination)
