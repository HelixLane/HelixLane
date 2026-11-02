#!/usr/bin/env python3
"""Exercise actual bounded process cleanup without sudo or a disposable OS user."""
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from native_fixture_cleanup import Process, finish_fixture, owned_processes, process, require_native_proc, signal_process, stop_owned_processes


class NativeFixtureCleanupTest(unittest.TestCase):
    def test_scope_includes_descendants_and_orphaned_uid_but_not_reused_pid(self):
        rows = [Process(10, 1, 100, 'S', (0,)), Process(11, 10, 110, 'S', (0,)),
                Process(12, 11, 120, 'S', (9001,)), Process(13, 1, 130, 'S', (9001,)),
                Process(20, 1, 200, 'S', (1000,)), Process(21, 1, 999, 'S', (0,))]
        owned = owned_processes({p.pid: p for p in rows}, {(10, 100), (21, 210)}, 9001)
        self.assertEqual(set(owned), {10, 11, 12, 13})

    def test_leaf_first_cleanup_escalates_and_preserves_unrelated_processes(self):
        rows = [Process(10, 1, 100, 'S', (0,)), Process(11, 10, 110, 'S', (0,)),
                Process(12, 11, 120, 'S', (9001,)), Process(13, 1, 130, 'S', (9001,)),
                Process(20, 1, 200, 'S', (1000,))]
        current = {p.pid: p for p in rows}
        signals = []

        def send(item, sig):
            signals.append((item.pid, sig))
            if sig == signal.SIGKILL or item.pid == 13:
                del current[item.pid]

        with patch('native_fixture_cleanup.require_native_proc'), \
             patch('native_fixture_cleanup.snapshot', side_effect=lambda: current.copy()), \
             patch('native_fixture_cleanup.signal_process', side_effect=send), \
             patch('native_fixture_cleanup.time.sleep'), \
             patch('native_fixture_cleanup.time.monotonic', side_effect=[0, 2, 3, 3, 3, 3]):
            stop_owned_processes([(10, 100)], uid=9001, grace=1)
        self.assertEqual(signals, [(12, signal.SIGTERM), (13, signal.SIGTERM),
                                   (12, signal.SIGKILL), (11, signal.SIGKILL), (10, signal.SIGKILL)])
        self.assertEqual(set(current), {20})

    def test_leftover_detached_child_is_killed_and_unrelated_process_survives(self):
        try:
            require_native_proc()
        except RuntimeError as error:
            if os.environ.get('HELIXLANE_REQUIRE_NATIVE_INTEGRATION') == '1':
                raise
            self.skipTest(str(error))
        with tempfile.TemporaryDirectory() as temp:
            marker = Path(temp) / 'child'
            child_code = ('import os,signal,time,pathlib; '
                          'signal.signal(signal.SIGTERM,signal.SIG_IGN); '
                          f'p=pathlib.Path({str(marker)!r}); t=p.with_suffix(".ready"); '
                          't.write_text(str(os.getpid())); t.rename(p); time.sleep(60)')
            parent_code = ('import signal,subprocess,sys; '
                           'signal.signal(signal.SIGTERM,signal.SIG_IGN); '
                           f'p=subprocess.Popen([sys.executable,"-c",{child_code!r}],start_new_session=True); '
                           'p.wait()')
            parent = subprocess.Popen([sys.executable, '-c', parent_code])
            unrelated = subprocess.Popen([sys.executable, '-c', 'import time;time.sleep(60)'])
            child_pid = None
            child_identity = None
            try:
                deadline = time.monotonic() + 5
                while not marker.exists() and time.monotonic() < deadline:
                    time.sleep(.02)
                self.assertTrue(marker.exists(), 'child startup timeout')
                child_pid = int(marker.read_text())
                child_identity = process(child_pid)
                identity = process(parent.pid).identity
                self.assertEqual(os.getsid(child_pid), child_pid, 'child escaped original session')
                stop_owned_processes([identity], grace=1)
                parent.wait(timeout=2)
                self.assertFalse(Path(f'/proc/{child_pid}').exists(), 'parent did not reap child')
                self.assertIsNone(unrelated.poll())
            finally:
                if child_identity is not None:
                    signal_process(child_identity, signal.SIGKILL)
                for spawned in [parent, unrelated]:
                    if spawned.poll() is None:
                        spawned.kill()
                    spawned.wait(timeout=2)

    def test_successful_cleanup_preserves_logs_and_removes_fixture(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / 'fixture'
            root.mkdir()
            (root / 'sshd.log').write_text('success evidence')
            logs = Path(temp) / 'logs'
            finish_fixture(root, [], logs)
            self.assertFalse(root.exists())
            self.assertEqual((logs / 'sshd.log').read_text(), 'success evidence')
            self.assertEqual((logs / 'cleanup.log').read_text(),
                             'Test/provisioning result: PASS\nCleanup: PASS\n')

    def test_cleanup_failure_preserves_logs_runs_remaining_steps_and_primary_error(self):
        for primary in [None, ValueError('original test failed')]:
            with self.subTest(primary=primary), tempfile.TemporaryDirectory() as temp:
                root = Path(temp) / 'fixture'
                root.mkdir()
                (root / 'sshd.log').write_text('fixture evidence\n')
                (root / 'client.json').write_text('private credentials must not be copied')
                logs = Path(temp) / 'logs'
                completed = []

                def fail():
                    raise RuntimeError('injected userdel failure')

                steps = [('userdel', fail), ('later cleanup', lambda: completed.append(True))]
                if primary is None:
                    with self.assertRaisesRegex(RuntimeError, 'injected userdel failure'):
                        finish_fixture(root, steps, logs, primary)
                else:
                    finish_fixture(root, steps, logs, primary)
                    self.assertIn('injected userdel failure', primary.__notes__[0])
                self.assertEqual(completed, [True])
                self.assertEqual((logs / 'sshd.log').read_text(), 'fixture evidence\n')
                self.assertIn('Cleanup: FAIL', (logs / 'cleanup.log').read_text())
                self.assertFalse((logs / 'client.json').exists())
                self.assertTrue(root.exists())


if __name__ == '__main__':
    unittest.main()
