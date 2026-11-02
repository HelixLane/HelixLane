#!/usr/bin/env python3
"""Bounded Linux fixture cleanup, scoped to captured processes and a disposable UID."""
import argparse
from dataclasses import dataclass
import os
from pathlib import Path
import shutil
import signal
import time


@dataclass(frozen=True)
class Process:
    pid: int
    parent: int
    started: int
    state: str
    uids: tuple

    @property
    def identity(self):
        return self.pid, self.started


def process(pid):
    directory = Path('/proc') / str(pid)
    fields = (directory / 'stat').read_text().rsplit(')', 1)[1].split()
    uid_line = next(line for line in (directory / 'status').read_text().splitlines()
                    if line.startswith('Uid:'))
    return Process(pid, int(fields[1]), int(fields[19]), fields[0],
                   tuple(map(int, uid_line.split()[1:])))


def snapshot():
    result = {}
    for directory in Path('/proc').iterdir():
        if directory.name.isdigit():
            try:
                item = process(int(directory.name))
                result[item.pid] = item
            except (FileNotFoundError, ProcessLookupError):
                pass  # Exited during the snapshot.
    return result


def owned_processes(current, identities, uid):
    owned = {p.pid: p for p in current.values()
             if p.identity in identities or (uid is not None and uid in p.uids)}
    while True:
        descendants = {p.pid: p for p in current.values() if p.parent in owned}
        if descendants.keys() <= owned.keys():
            return owned
        owned.update(descendants)


def signal_process(item, sig):
    # Pin the process before checking its birth time; never signal a reused PID.
    try:
        descriptor = os.pidfd_open(item.pid)
    except ProcessLookupError:
        return
    try:
        if process(item.pid).identity == item.identity:
            signal.pidfd_send_signal(descriptor, sig)
    except (FileNotFoundError, ProcessLookupError):
        pass
    finally:
        os.close(descriptor)


def require_native_proc():
    actual = int(Path('/proc/self/stat').read_text().split(' ', 1)[0])
    if actual != os.getpid():
        raise RuntimeError(f'Native /proc PID namespace required: getpid={os.getpid()}, proc={actual}')


def stop_owned_processes(roots, uid=None, grace=5):
    require_native_proc()
    if uid is not None and uid <= 0:
        raise ValueError('Refusing non-disposable UID')
    identities = set(roots)
    for sig in (signal.SIGTERM, signal.SIGKILL):
        deadline = time.monotonic() + grace
        signaled = set()
        while True:
            owned = owned_processes(snapshot(), identities, uid)
            identities.update(p.identity for p in owned.values())
            live = [p for p in owned.values() if p.state != 'Z']
            # Session zombies must be reaped before userdel. Wrapper zombies are
            # reaped by the provisioning process's Popen.wait after this helper.
            user_processes = [p for p in owned.values() if uid is not None and uid in p.uids]
            if not live and not user_processes:
                return
            # Stop leaves first so privileged monitors can reap their user
            # children before the monitors/listeners themselves are terminated.
            parents = {p.parent for p in owned.values()}
            for item in live:
                if item.pid not in parents and item.identity not in signaled:
                    signal_process(item, sig)
                    signaled.add(item.identity)
            if time.monotonic() >= deadline:
                break
            time.sleep(.05)
    raise RuntimeError(f'Fixture processes remain after TERM/KILL deadlines: {sorted(owned)}')


def finish_fixture(root, steps, logs, primary_error=None):
    """Attempt every cleanup step and preserve only logs, even when cleanup fails."""
    errors = []
    for label, action in steps:
        try:
            action()
        except Exception as error:
            errors.append(f'{label}: {type(error).__name__}: {error}')
    try:
        logs.mkdir(parents=True, exist_ok=True)
    except Exception as error:
        errors.append(f'Create log directory: {type(error).__name__}: {error}')
    for log in root.glob('*.log'):
        try:
            shutil.copy2(log, logs / log.name)
        except Exception as error:
            errors.append(f'Preserve {log.name}: {type(error).__name__}: {error}')
    result = f'Test/provisioning result: {"FAIL" if primary_error else "PASS"}\n'
    try:
        (logs / 'cleanup.log').write_text(result + (
            'Cleanup: FAIL\n' + '\n'.join(errors) if errors else 'Cleanup: PASS') + '\n')
    except Exception as error:
        errors.append(f'Preserve cleanup result: {type(error).__name__}: {error}')
    if not errors:
        try:
            shutil.rmtree(root)
        except Exception as error:
            errors.append(f'Remove fixture directory: {type(error).__name__}: {error}')
            try:
                (logs / 'cleanup.log').write_text(result + 'Cleanup: FAIL\n' + '\n'.join(errors) + '\n')
            except OSError:
                pass  # The original cleanup failure is still raised below.
    if errors:
        message = f'Fixture cleanup failed; retained directory {root}: ' + '; '.join(errors)
        if primary_error is not None:
            primary_error.add_note(message)  # Preserve the original test exception.
        else:
            raise RuntimeError(message)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', action='append', default=[])
    parser.add_argument('--uid', type=int, required=True)
    args = parser.parse_args()
    roots = [tuple(map(int, value.split(':'))) for value in args.root]
    stop_owned_processes(roots, args.uid)


if __name__ == '__main__':
    main()
