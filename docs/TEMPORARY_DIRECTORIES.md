# Temporary plugin data and test caches

Each plugin host owns a root under Zellij's cache directory, in `plugin-tmp/`.
On Linux this is normally `~/.cache/zellij/plugin-tmp/`.
Each loading context owns a flat temporary directory inside that root.
Plugin environments and workers share its ownership.

Guest `/data` stays available through `BeforeClose` and reload.
The directory disappears after its last user finishes.
The host root disappears after all its plugin work finishes, including work that outlives the plugin thread.
Cleanup uses the remaining shutdown budget. It does not extend the shutdown deadline.
Forced termination leaves cleanup for the next host startup.

Each root has a held file lock in a sibling `.lease` file.
A permanent registry lock serializes creation and reclamation.
Startup removes only roots whose lease locks it can acquire.
Cleanup does not follow symlinks or use directory age or a PID to decide ownership.

Persistent guest `/cache`, `permissions.kdl`, and session resurrection data keep their existing locations and lifetimes.
No plugin API or wire format changes are required.

## Test runners

`cargo x test` owns a temporary run directory for unit tests and doctests.
`cargo x integration-test` and the host-side remote, Firecracker, Postgres, and e2e test commands use the same ownership.
The runner prints the path, then removes it after its children exit, including failures and terminated children.
Read failure output during the run. These directories are not retained for later inspection.

The runner sets `ZELLIJ_TEST_CACHE_ROOT` for its children.
Zellij resolves its cache to `<root>/<process-id>/cache` before other cache paths initialize.
This override works on Linux and macOS without changing `HOME`, Cargo's cache, or `target/`.
Integration tests also place their isolated process roots beneath the run directory.
Their existing `HOME`, socket, log, and XDG isolation stays in effect.

Run roots live under `/tmp/zj-runs-<uid>/` to keep Unix socket paths short.
The runner passes its lock descriptor to descendants.
If the runner dies while a test child remains alive, that child still protects the run from reclamation.
The next runner reclaims abandoned runs after all lock holders exit.

For direct Cargo or nextest use, supply an absolute directory yourself:

```sh
ZELLIJ_TEST_CACHE_ROOT=/tmp/my-zellij-tests cargo test -p zellij-plugin-host
ZELLIJ_TEST_CACHE_ROOT=/tmp/my-zellij-tests cargo nextest run -p zellij-integration-tests -j 16
```

Direct commands isolate caches but do not own or automatically remove the supplied directory.
Remove it after all their test processes exit. Use `cargo x` for automatic cleanup and stale-run recovery.

## Offline cleanup of legacy UUID directories

Old versions created unmarked UUID directories directly in the Zellij cache.
Startup does not remove these directories because they have no ownership locks.

1. Stop all Zellij sessions, test runners, and test children.
2. Set `cache` below to the actual Zellij cache directory.
3. Run the script with `delete = False` to review the directories.
4. Set `delete = True` and run it again after you confirm the list.

```python
from pathlib import Path
import re
import shutil

cache = Path.home() / ".cache" / "zellij"  # Linux default. Adjust for macOS or XDG overrides.
delete = False
uuid_name = re.compile(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}")

for path in sorted(cache.iterdir()):
    if uuid_name.fullmatch(path.name) and not path.is_symlink() and path.is_dir():
        print(path)
        if delete:
            shutil.rmtree(path)
```

This procedure selects only UUID-named directories directly under the cache.
It leaves plugin caches, permissions, resurrection data, and `plugin-tmp/` alone.
