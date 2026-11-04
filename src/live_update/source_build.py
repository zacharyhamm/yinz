"""Visible build worker; all paths and commands come from a pinned operation record."""
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import subprocess
import sys


def save(path, value):
    temporary = path.with_suffix(".tmp")
    with temporary.open("w") as output:
        json.dump(value, output)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)


def incarnation(pid):
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]
    except FileNotFoundError:
        return None


def stop(path):
    if not path.exists():
        return
    process = json.loads(path.read_text())
    if incarnation(process["pid"]) == process["start"]:
        try:
            os.killpg(process["pid"], signal.SIGKILL)
        except ProcessLookupError:
            pass


def build(record_path):
    record = json.loads(record_path.read_text())
    directory = record_path.parent
    process_path = directory / "process.json"
    child = None
    cancelled = False

    def cancellation_requested():
        return cancelled or (directory / "cancel").exists() or (directory.parent / "cancel").exists()

    def interrupt(signum, frame):
        nonlocal cancelled
        cancelled = True
        stop(process_path)

    for signum in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):
        signal.signal(signum, interrupt)
    with open(record["log"], "ab", buffering=0) as log:
        def write(data):
            log.write(data)
            try:
                sys.stdout.buffer.write(data)
                sys.stdout.buffer.flush()
            except OSError:
                interrupt(signal.SIGHUP, None)
        try:
            write(f"Building local main {record['commit']}\n".encode())
            lock_path = Path(record["snapshot"]) / "Cargo.lock"
            if not lock_path.is_file():
                raise RuntimeError("pinned source is missing Cargo.lock")
            pinned_lock = lock_path.read_bytes()
            for tool in ("cargo", "cargo-zigbuild", "zig", "protoc", "python3", "file", "readelf"):
                if not shutil.which(tool):
                    raise RuntimeError(f"source build requires {tool} on the server host")
            if "CARGO_TARGET_DIR" in os.environ:
                raise RuntimeError("source builds do not accept CARGO_TARGET_DIR")
            installed = subprocess.check_output(["rustup", "target", "list", "--installed"], text=True).splitlines()
            for target in record["targets"]:
                if target not in installed:
                    raise RuntimeError(f"install server build target first: rustup target add {target}")
            for target in record["targets"]:
                if cancellation_requested():
                    raise RuntimeError("source build cancelled")
                write(f"\nCompiling {target}\n".encode())
                # The outer xtask build must also ignore native mold flags;
                # source-build clears them again for Zig's target compilation.
                environment = dict(os.environ, CARGO_ENCODED_RUSTFLAGS="")
                child = subprocess.Popen(["cargo", "x", "source-build", target], cwd=record["snapshot"],
                                         env=environment, stdout=subprocess.PIPE,
                                         stderr=subprocess.STDOUT, start_new_session=True)
                save(process_path, {"pid": child.pid, "start": incarnation(child.pid)})
                if cancellation_requested():
                    stop(process_path)
                with selectors.DefaultSelector() as selector:
                    selector.register(child.stdout, selectors.EVENT_READ)
                    while selector.get_map():
                        for key, _ in selector.select(timeout=0.1):
                            data = os.read(key.fd, 65536)
                            if data:
                                write(data)
                            else:
                                selector.unregister(key.fd)
                        if cancellation_requested():
                            stop(process_path)
                child.stdout.close()
                status = child.wait()
                if cancellation_requested() or status:
                    raise RuntimeError(f"source build {'cancelled' if cancellation_requested() else 'failed'} for {target} (exit {status})")
                if not lock_path.is_file() or lock_path.read_bytes() != pinned_lock:
                    raise RuntimeError("pinned Cargo.lock changed during source build; refusing to publish artifacts")
            if cancellation_requested():
                raise RuntimeError("source build cancelled")
            write(b"\nSource artifacts built successfully. Validating before session replacement.\n")
            save(directory / "result.json", {"success": True})
        except Exception as error:
            stop(process_path)
            if child is not None:
                child.stdout.close()
                child.wait()
            write(f"\nSource update stopped: {error}\nLog: {record['log']}\n".encode())
            save(directory / "result.json", {"success": False, "error": str(error)})
            return 1
    return 0


if __name__ == "__main__":
    if sys.argv[1] == "--stop":
        process_path = Path(sys.argv[2])
        (process_path.parent / "cancel").touch()
        stop(process_path)
    else:
        sys.exit(build(Path(sys.argv[1])))
