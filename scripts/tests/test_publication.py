"""Publication must use current allowlisted source, never private substitutions."""
from pathlib import Path
import runpy
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class PublicationTests(unittest.TestCase):
    def test_snapshot_ignores_private_overrides_and_nested_work_files(self):
        namespace = runpy.run_path(str(ROOT / 'scripts/prepare-distribution.py'))
        prepare = namespace['prepare']
        policy = prepare.__globals__
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for relative, contents in {
                'README.md': 'current source',
                'source/ui/src/main.tsx': 'current UI',
                'source/ui/.local/private.txt': 'private',
                'source/ui/dist-experiments/assets/test.js': 'generated',
                '.local/distribution-overrides/README.md': 'obsolete override',
            }.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(contents)
            policy.update(ROOT=root, FILES=['README.md'], TREES=['source/ui'], SCRIPTS=[], DOCS=[])
            target = root / 'snapshot'
            prepare(target)
            self.assertEqual((target / 'README.md').read_text(), 'current source')
            self.assertEqual((target / 'source/ui/src/main.tsx').read_text(), 'current UI')
            self.assertFalse((target / 'source/ui/.local').exists())
            self.assertFalse((target / 'source/ui/dist-experiments').exists())
            self.assertIn('README.md', (target / 'SOURCE-MANIFEST.sha256').read_text())
            with self.assertRaises(SystemExit):
                prepare(target)

    def test_heading_anchors_match_current_public_links(self):
        anchors = runpy.run_path(str(ROOT / 'scripts/check-docs.py'))['anchors']
        self.assertEqual(anchors('# Hello\n## Hello\n## Audio & files\n'), {'hello', 'hello-1', 'audio--files'})


if __name__ == '__main__':
    unittest.main()
