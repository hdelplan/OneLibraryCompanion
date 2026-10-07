#!/usr/bin/env python3
"""Check publication guides for missing local links and excluded product documentation."""
from pathlib import Path
import re
import runpy
from urllib.parse import unquote, urlsplit

root = Path(__file__).resolve().parent.parent
policy = runpy.run_path(str(root / 'scripts/prepare-distribution.py'))
pages = [root / 'README.md', *(root / 'docs' / name for name in policy['DOCS'])]
errors = []
for page in pages:
    text = page.read_text()
    if re.search(r'\btest screen\b|\bexperiments?\b', text, re.I):
        errors.append(f'{page.relative_to(root)}: documentation outside distribution scope')
    for target in re.findall(r'!?\[[^\]]*\]\(([^\s)]+)(?:\s+[^)]*)?\)', text):
        link = urlsplit(target.strip('<>'))
        if link.scheme or link.netloc or not link.path:
            continue
        resolved = (page.parent / unquote(link.path)).resolve()
        if not resolved.is_relative_to(root) or not resolved.exists():
            errors.append(f'{page.relative_to(root)}: missing local link {target}')
if errors:
    raise SystemExit('\n'.join(errors))
print(f'Publication documentation checks passed ({len(pages)} pages)')
