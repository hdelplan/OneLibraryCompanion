#!/usr/bin/env python3
"""Validate public guide links, scope and release-version consistency."""
import json
from pathlib import Path
import plistlib
import re
import runpy
import tomllib
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parent.parent


def anchors(text):
    headings = re.findall(r'^#{1,6}\s+(.+?)\s*#*$', text, re.M)
    used = {}
    result = set()
    for heading in headings:
        slug = re.sub(r'[^\w\- ]', '', heading.lower()).replace(' ', '-')
        count = used.get(slug, 0)
        used[slug] = count + 1
        result.add(f'{slug}-{count}' if count else slug)
    return result


def check(root=ROOT):
    policy = runpy.run_path(str(root / 'scripts/prepare-distribution.py'))
    pages = [root / 'README.md', *(root / 'docs' / name for name in policy['DOCS'])]
    public_pages = set(p.resolve() for p in pages)
    errors = []
    for page in pages:
        text = page.read_text()
        if page.name != 'development.md' and re.search(r'\btest screen\b|\bexperiments?\b', text, re.I):
            errors.append(f'{page.relative_to(root)}: development-only content in user guide')
        scoped = text
        if page.name == 'distribution.md':
            scoped = re.sub(r'^## Raspberry Pi headless\n.*?(?=^## |\Z)', '', text, flags=re.M | re.S)
        if re.search(r'\bipad(?:os)?\b|\bios\b', scoped, re.I):
            errors.append(f'{page.relative_to(root)}: tablet documentation belongs only in headless setup')
        if re.search(r'/Users/|\.local/|10\.80\.1\.|\bBuild \d+\b|has been removed|was fixed', text):
            # The development data directory is an intentional public setting.
            sanitized = text.replace('`.local/app-data`', '').replace('~/.local/', '~/')
            if re.search(r'/Users/|\.local/|10\.80\.1\.|\bBuild \d+\b|has been removed|was fixed', sanitized):
                errors.append(f'{page.relative_to(root)}: private path or development history in public guide')
        for target in re.findall(r'!?\[[^\]]*\]\(([^\s)]+)(?:\s+[^)]*)?\)', text):
            link = urlsplit(target.strip('<>'))
            if link.scheme or link.netloc:
                continue
            resolved = (page.parent / unquote(link.path)).resolve() if link.path else page.resolve()
            if not resolved.is_relative_to(root) or not resolved.exists():
                errors.append(f'{page.relative_to(root)}: missing local link {target}')
                continue
            if resolved.suffix == '.md' and resolved.parent == root / 'docs' and resolved not in public_pages:
                errors.append(f'{page.relative_to(root)}: link excluded from public snapshot: {target}')
            if link.fragment and resolved.suffix == '.md' and unquote(link.fragment) not in anchors(resolved.read_text()):
                errors.append(f'{page.relative_to(root)}: missing section {target}')
    version = tomllib.loads((root / 'source/host/Cargo.toml').read_text())['package']['version']
    ui = json.loads((root / 'source/ui/package.json').read_text())['version']
    lock = json.loads((root / 'source/ui/package-lock.json').read_text())
    with (root / 'source/desktop/macos/Info.plist').open('rb') as file:
        mac = plistlib.load(file)['CFBundleShortVersionString']
    for name, value in [('UI', ui), ('UI lock', lock['version']), ('UI package lock', lock['packages']['']['version']), ('Mac', mac)]:
        if value != version:
            errors.append(f'{name} version {value} does not match host {version}')
    if not (root / 'docs/release-notes.md').read_text().startswith(f'# OneLibraryCompanion {version}\n'):
        errors.append('Release notes version does not match host')
    workflow = (root / '.github/workflows/release.yml').read_text()
    if f'default: v{version}\n' not in workflow:
        errors.append('Release workflow default does not match host')
    if errors:
        raise SystemExit('\n'.join(errors))
    print(f'Public documentation and version checks passed ({len(pages)} pages, {version})')


if __name__ == '__main__':
    check()
