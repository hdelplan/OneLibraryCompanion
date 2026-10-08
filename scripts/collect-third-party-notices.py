#!/usr/bin/env python3
"""Collect pinned Cargo and installed npm notices without downloading anything."""
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / 'third-party-licenses'
PREFIXES = ('license', 'licence', 'copying', 'notice', 'copyright', 'unlicense')


def notices(directory):
    return sorted(p for p in directory.iterdir()
                  if p.is_file() and p.name.lower().startswith(PREFIXES))


def main():
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--format-version', '1'], cwd=ROOT))
    packages = metadata['packages']
    by_name = {p['name']: p for p in packages}
    inventory = []
    texts = []
    for package in sorted(packages, key=lambda p: (p['name'], p['version'])):
        if package['name'].startswith('pioneer-companion-'):
            continue
        directory = Path(package['manifest_path']).parent
        files = notices(directory)
        source_note = 'Files from the pinned crate source.'
        if package['name'] in ('binrw', 'binrw_derive'):
            files = [OUT / 'binrw-LICENSE-MIT.txt']
            source_note = 'Upstream v0.15.2 LICENSE (shared workspace license).'
        elif package['name'] == 'realfft':
            files = [OUT / 'realfft-NOTICE.txt']
            source_note = 'Pinned upstream MIT declaration and author; standard MIT permission text.'
        elif package['name'].startswith('winapi-') and not files:
            files = notices(Path(by_name['winapi']['manifest_path']).parent)
            source_note = 'winapi workspace licenses; Windows target support crate.'
        if not files:
            raise SystemExit(f'Missing Rust notice: {package["name"]}')
        entry = {'ecosystem': 'cargo', 'name': package['name'], 'version': package['version'],
                 'license': package['license'], 'repository': package.get('repository'),
                 'notice_files': [p.name for p in files], 'provenance': source_note}
        inventory.append(entry)
        texts.append(f'\n{"=" * 72}\n{entry["name"]} {entry["version"]}\nLicense: {entry["license"]}\nSource: {entry["repository"]}\n{source_note}\n')
        for file in files:
            texts.append(f'\n--- {file.name} ---\n{file.read_text()}\n')
    (OUT / 'CARGO-NOTICES.txt').write_text('Pinned Cargo dependency notices (all lockfile targets, including build dependencies).\n' + ''.join(texts))

    lock = json.loads((ROOT / 'source/ui/package-lock.json').read_text())
    texts = []
    for location, locked in sorted(lock['packages'].items()):
        if not location:
            continue
        directory = ROOT / 'source/ui' / location
        # Platform-specific optional build tools absent on this host are not shipped.
        if not directory.exists():
            if locked.get('optional') and locked.get('dev'):
                continue
            raise SystemExit(f'Missing npm package: {location}; run npm ci first')
        package = json.loads((directory / 'package.json').read_text())
        files = notices(directory)
        provenance = 'Files from the installed, lockfile-pinned npm package.'
        if not files and package['name'].startswith('@esbuild/'):
            files = notices(ROOT / 'source/ui/node_modules/esbuild')
            provenance = 'esbuild workspace license for the optional platform executable.'
        if not files and package['name'].startswith('@rollup/'):
            files = notices(ROOT / 'source/ui/node_modules/rollup')
            provenance = 'Rollup workspace license for the optional platform executable.'
        if not files:
            raise SystemExit(f'Missing npm notice: {location}')
        repository = package.get('repository', package.get('homepage'))
        if isinstance(repository, dict):
            repository = repository.get('url')
        entry = {'ecosystem': 'npm', 'name': package['name'], 'version': locked['version'],
                 'license': package.get('license', package.get('licenses')), 'repository': repository,
                 'development_only': bool(locked.get('dev')), 'notice_files': [p.name for p in files],
                 'provenance': provenance}
        inventory.append(entry)
        texts.append(f'\n{"=" * 72}\n{entry["name"]} {entry["version"]}\nLicense: {entry["license"]}\nSource: {repository}\n{provenance}\n')
        for file in files:
            texts.append(f'\n--- {file.name} ---\n{file.read_text()}\n')
    (OUT / 'NPM-NOTICES.txt').write_text('Installed npm dependency notices, including production dependencies and local build tools.\nAbsent optional build tools for other platforms are not included.\n' + ''.join(texts))
    (OUT / 'dependency-inventory.json').write_text(json.dumps(inventory, indent=2) + '\n')
    print(f'Collected notices for {len(inventory)} pinned dependencies.')


if __name__ == '__main__':
    main()
