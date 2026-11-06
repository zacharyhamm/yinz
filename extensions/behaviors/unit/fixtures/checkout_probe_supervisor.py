"""Linux foreground process-tree collection for the opt-in checkout probe.

This is test cleanup, not a sandbox. Subreaper adoption also covers children
which leave the initial process group or outlive the command's immediate parent.
"""

import ctypes
import os
import signal
import sys
import time


def supervise(timeout, command):
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))

    interrupted = None

    def interrupt(signum, _frame):
        nonlocal interrupted
        interrupted = signum

    signal.signal(signal.SIGTERM, interrupt)
    signal.signal(signal.SIGINT, interrupt)
    deadline = time.monotonic() + timeout
    leader = os.fork()
    if leader == 0:
        try:
            signal.signal(signal.SIGTERM, signal.SIG_DFL)
            signal.signal(signal.SIGINT, signal.SIG_DFL)
            os.setsid()
            os.execvp(command[0], command)
        except OSError as error:
            print(f"probe command could not start: {error}", file=sys.stderr, flush=True)
            os._exit(125)

    children_file = f"/proc/self/task/{os.getpid()}/children"
    leader_code = 125
    timed_out = False
    collected = False

    def terminate_children():
        # Only signal unreaped direct children, whose PIDs cannot be reused.
        # Killing parents reparents their descendants here; the next pass kills
        # those too, including double-forked and setsid descendants.
        with open(children_file, encoding="ascii") as children:
            for child in children.read().split():
                os.kill(int(child), signal.SIGKILL)

    try:
        while True:
            if time.monotonic() >= deadline:
                timed_out = True
            if timed_out or interrupted is not None:
                terminate_children()
            try:
                pid, status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                collected = True
                break
            if pid:
                if pid == leader:
                    leader_code = os.waitstatus_to_exitcode(status)
                continue
            time.sleep(0.01)
    finally:
        # Unexpected supervisor exceptions must also collect the entire tree.
        while not collected:
            terminate_children()
            try:
                pid, _status = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                break
            if not pid:
                time.sleep(0.01)

    if interrupted is not None:
        return 128 + interrupted
    if timed_out:
        print(
            f"probe process tree exceeded {timeout:g}s deadline; all descendants collected",
            file=sys.stderr,
        )
        return 124
    return leader_code if leader_code >= 0 else 128 - leader_code


if __name__ == "__main__":
    sys.exit(supervise(float(sys.argv[1]), sys.argv[2:]))
