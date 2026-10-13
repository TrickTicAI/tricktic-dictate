#!/usr/bin/env python3
"""Native regression: killing the owning app must remove its private worker tree."""
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest


@unittest.skipUnless(sys.platform == 'darwin', 'macOS kqueue supervisor')
class Ownership(unittest.TestCase):
    def test_parent_crash_kills_worker(self):
        with tempfile.TemporaryDirectory(prefix='writer-host-test-') as folder:
            root = Path(folder)
            host = root / 'writer-worker-host'
            subprocess.run(['cc', '-O2', '-Wall', '-Wextra', '-Werror',
                            str(Path(__file__).with_name('worker-host-macos.c')), '-o', str(host)], check=True)
            receipt = root / 'pid'
            code = ('import subprocess,time,pathlib\n'
                    f'p=subprocess.Popen([{str(host)!r},"/bin/sleep","120"],process_group=0)\n'
                    f'pathlib.Path({str(receipt)!r}).write_text(str(p.pid))\n'
                    'time.sleep(120)\n')
            parent = subprocess.Popen([sys.executable, '-c', code])
            try:
                until = time.monotonic() + 5
                while not receipt.exists() and time.monotonic() < until:
                    time.sleep(.05)
                host_pid = receipt.read_text()
                time.sleep(.2)
                children = subprocess.check_output(['pgrep', '-P', host_pid], text=True).split()
                self.assertTrue(children)
                parent.kill()
                parent.wait()
                live = children + [host_pid]
                until = time.monotonic() + 5
                while live and time.monotonic() < until:
                    remaining = []
                    for pid in live:
                        state = subprocess.run(['ps', '-o', 'stat=', '-p', pid], capture_output=True, text=True).stdout.strip()
                        if state and not state.startswith('Z'):
                            remaining.append(pid)
                    live = remaining
                    time.sleep(.05)
                self.assertEqual(live, [])
            finally:
                if parent.poll() is None:
                    parent.kill()
                    parent.wait()


if __name__ == '__main__':
    unittest.main()
