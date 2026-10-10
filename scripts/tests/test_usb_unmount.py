import importlib.util
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('usb', Path(__file__).resolve().parents[2] / 'packaging/olc-usb.py')
usb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(usb)


class UnmountTests(unittest.TestCase):
    def test_inventory_excludes_system_and_network_mounts(self):
        tree = {'blockdevices': [
            {'name': '/dev/mmcblk0', 'children': [{'name': '/dev/mmcblk0p7', 'mountpoints': ['/']}]},
            {'name': '/dev/sda', 'tran': 'usb', 'children': [{'name': '/dev/sda1', 'label': 'DJTT', 'mountpoints': ['/media/me/DJTT']}]},
            {'name': '/dev/sdb', 'tran': 'usb', 'mountpoints': ['/']},
            {'name': '/dev/sdc', 'tran': 'usb', 'mountpoints': ['/media/me/USB', '/etc']},
        ]}
        with patch.object(usb.subprocess, 'check_output', return_value=json.dumps(tree)):
            self.assertEqual([v['device'] for v in usb.volumes()], ['/dev/sda'])

    def run_unmount(self, decks, unmount_code=0):
        response = Mock()
        response.__enter__ = Mock(return_value=response)
        response.__exit__ = Mock(return_value=False)
        response.read.return_value = json.dumps({'decks': decks, 'directPeers': []})
        calls = []
        reports = []

        def run(args, **kwargs):
            calls.append(args)
            return subprocess.CompletedProcess(args, unmount_code if 'unmount' in args else 0, '', 'Device is busy' if unmount_code else '')

        with patch.object(usb, 'volumes', side_effect=[[{'device': '/dev/sda', 'blocks': ['/dev/sda1']}], []]), \
             patch.object(usb.urllib.request, 'urlopen', return_value=response), \
             patch.object(usb.subprocess, 'run', side_effect=run), \
             patch.object(usb, 'report', side_effect=lambda *args: reports.append(args)):
            usb.unmount('/dev/sda', 'http://127.0.0.1:8787/api/live', 'test')
        return calls, reports

    def test_connected_deck_blocks_before_stopping_service(self):
        calls, reports = self.run_unmount([{'number': 1}])
        self.assertEqual(calls, [])
        self.assertEqual(reports[-1][1], 'error')

    def test_busy_usb_restarts_olc_without_claiming_success(self):
        calls, reports = self.run_unmount([], 1)
        self.assertEqual(calls[-1][2], 'start')
        self.assertEqual(reports[-1][1], 'error')
        self.assertIn('busy', reports[-1][2])
        self.assertNotIn('--force', calls[1])

    def test_success_is_verified_before_safe_to_remove(self):
        calls, reports = self.run_unmount([])
        self.assertEqual(calls[0][2], 'stop')
        self.assertEqual(calls[-1][2], 'start')
        self.assertEqual(reports[-1][1], 'done')


if __name__ == '__main__':
    unittest.main()
