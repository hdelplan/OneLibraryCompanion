#!/usr/bin/env python3
"""Package an existing ELF host for Debian, on Linux or a cross-build machine."""
import argparse
import gzip
import hashlib
import io
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent


def archive(root, exclude=()):
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode='w', format=tarfile.GNU_FORMAT) as tar:
        for path in sorted(root.rglob('*')):
            relative = path.relative_to(root)
            if relative.parts[0] in exclude:
                continue
            info = tar.gettarinfo(str(path), './' + relative.as_posix())
            info.uid = info.gid = 0
            info.uname = info.gname = 'root'
            info.mtime = int(os.environ.get('SOURCE_DATE_EPOCH', '0'))
            info.mode = 0o755 if path.is_dir() or (info.isfile() and path.stat().st_mode & 0o111) else 0o644
            if info.issym():
                info.mode = 0o777
            if info.isfile():
                with path.open('rb') as source:
                    tar.addfile(info, source)
            else:
                tar.addfile(info)
    return gzip.compress(data.getvalue(), mtime=0)


def deb(root, destination):
    # deb(5): common ar, then version, control tar, data tar in this order.
    entries = [('debian-binary', b'2.0\n'), ('control.tar.gz', archive(root / 'DEBIAN')), ('data.tar.gz', archive(root, ('DEBIAN',)))]
    with destination.open('wb') as output:
        output.write(b'!<arch>\n')
        for name, data in entries:
            output.write(f'{name:<16}{0:<12}{0:<6}{0:<6}{"100644":<8}{len(data):<10}`\n'.encode('ascii'))
            output.write(data)
            if len(data) % 2:
                output.write(b'\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--arch', choices=['arm64', 'amd64'], required=True)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    os.chdir(ROOT)
    binary = args.binary.read_bytes()
    assert binary[:6] == b'\x7fELF\x02\x01', 'Expected a little-endian 64-bit Linux ELF binary'
    assert struct.unpack_from('<H', binary, 18)[0] == {'arm64': 183, 'amd64': 62}[args.arch], 'ELF architecture mismatch'
    versions = [tuple(map(int, item.split(b'.'))) for item in re.findall(rb'GLIBC_(\d+\.\d+(?:\.\d+)?)', binary)]
    assert not versions or max(versions) <= (2, 36), 'Binary requires a glibc newer than Debian Bookworm'
    version = tomllib.loads(Path('source/host/Cargo.toml').read_text())['package']['version']
    base = Path('builds') / f'linux-{args.arch}'
    base.mkdir(parents=True, exist_ok=True)
    releases = Path('builds/releases')
    releases.mkdir(parents=True, exist_ok=True)
    outputs = []
    for desktop in [False, True]:
        name = 'onelibrarycompanion' if desktop else 'onelibrarycompanion-host'
        root = base / name
        if root.exists():
            shutil.rmtree(root)
        (root / 'DEBIAN').mkdir(parents=True)
        lib = root / 'usr/lib/onelibrarycompanion'
        lib.mkdir(parents=True)
        doc = root / f'usr/share/doc/{name}'
        doc.mkdir(parents=True)
        shutil.copy2('LICENSE', doc / 'copyright')
        shutil.copy2('THIRD_PARTY_NOTICES.md', doc / 'THIRD_PARTY_NOTICES.md')
        shutil.copy2('docs/distribution.md', doc / 'README.md')
        if desktop:
            shutil.copy2('source/desktop/linux/olc.py', lib / 'olc.py')
            (lib / 'olc.py').chmod(0o755)
            bin_dir = root / 'usr/bin'
            bin_dir.mkdir(parents=True)
            (bin_dir / 'olc').symlink_to('../lib/onelibrarycompanion/olc.py')
            apps = root / 'usr/share/applications'
            apps.mkdir(parents=True)
            shutil.copy2('packaging/onelibrarycompanion.desktop', apps / 'onelibrarycompanion.desktop')
            icons = root / 'usr/share/icons/hicolor/scalable/apps'
            icons.mkdir(parents=True)
            shutil.copy2('packaging/olc.svg', icons / 'onelibrarycompanion.svg')
            deps = f'onelibrarycompanion-host (= {version}), python3, python3-gi, gir1.2-gtk-3.0, gir1.2-webkit2-4.1'
        else:
            shutil.copy2('packaging/olc-usb.py', lib / 'olc-usb.py')
            shutil.copy2(args.binary, lib / 'olc-host')
            (lib / 'olc-host').chmod(0o755)
            shutil.copytree('source/ui/dist', lib / 'dist')
            shutil.copytree('third-party-licenses', doc / 'third-party-licenses')
            units = root / 'usr/lib/systemd/user'
            units.mkdir(parents=True)
            shutil.copy2('packaging/olc-host.service', units / 'olc-host.service')
            shutil.copy2('packaging/olc-host.postinst', root / 'DEBIAN/postinst')
            (root / 'DEBIAN/postinst').chmod(0o755)
            deps = 'libc6 (>= 2.36), libgcc-s1, libcap2-bin, python3, udisks2, util-linux, sudo'
        (root / 'DEBIAN/control').write_text(f'Package: {name}\nVersion: {version}\nArchitecture: {args.arch}\nMaintainer: OLC maintainers <noreply@onelibrarycompanion.local>\nSection: sound\nPriority: optional\nDepends: {deps}\nDescription: OneLibraryCompanion CDJ companion\n Native and LAN interfaces for CDJ libraries, status and set history.\n')
        output = releases / f'{name}_{version}_{args.arch}.deb'
        if shutil.which('dpkg-deb'):
            subprocess.run(['dpkg-deb', '--build', '--root-owner-group', str(root), str(output)], check=True)
        else:
            deb(root, output)
        outputs.append(output)
        print(output)
    (releases / f'SHA256SUMS-linux-{args.arch}.txt').write_text(''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n' for p in outputs))
    print(f'ELF architecture and glibc baseline checked (highest GLIBC version: {max(versions) if versions else "none"}).')


if __name__ == '__main__':
    main()
