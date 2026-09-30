# Render benchmarks and notcurses profiling

## Graphics microbenchmark

Run from the worktree root:

```sh
cargo x bench -- kitty_graphics --save-baseline before
# After changing the render path:
cargo x bench -- kitty_graphics --baseline before
```

The `kitty_graphics` group uses one 200-column, 50-row pane with 8×16-pixel cells
and Kitty graphics support. Each iteration replaces an 800×480 RGB image.
Two deterministic images alternate. Each transmission uses 4096-byte base64
chunks. The old image is deleted, including in the ingest-only measurement.

| Measurement | Timed work | Throughput denominator |
| --- | --- | --- |
| `ingest` | Parse graphics input and update the grid | Input bytes |
| `frame` | Ingest, render, and serialize | Input bytes |
| `render_only` | Render and serialize, with ingest outside the timer | Serialized bytes |

The benchmark prints input and serialized byte counts. Its startup assertions
require a complete image transmission for both image variants. For a quick check:

```sh
cargo x bench -- kitty_graphics --test
```

This is a synthetic direct-transmission workload, not a recording of notcurses.
It does not measure IPC, queue growth, or terminal consumption. Use live profiles
to investigate those stages. Existing text benchmarks remain available through
`cargo x bench`.

## Build for live profiles

Keep the worktree's own `target/` directory. Build optimized code with line tables
and symbols retained:

```sh
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only \
CARGO_PROFILE_RELEASE_STRIP=none cargo x build --release
```

Use `target/release/yinz` for the test session. Keep Ghostty settings, Yinz
configuration, terminal dimensions, and notcurses version fixed between runs.
Use one pane and one demo process. Record `git rev-parse HEAD`, `git status --short`,
`stty size`, `ghostty --version`, and `notcurses-demo --version` with the results.

Run the demo inside that pane, using a different file for each run:

```sh
mkdir -p /tmp/yinz-notcurses-results
/usr/bin/time -p notcurses-demo -c -J /tmp/yinz-notcurses-results/before-1.json
```

Collect three unprofiled runs before and after a change. Record completion or
disconnect and elapsed time. If the client disconnects while the demo continues
in the server, record time to disconnect separately from the demo's duration.

## Profile the server and client

Find the two processes for the dedicated test session from another terminal:

```sh
ps -eo pid,ppid,args | rg 'yinz'
```

Select the server with `--server` and the attached client for that session.
Start the capture just before the graphics-heavy interval:

```sh
# Replace these example PIDs with the test session's server and client PIDs.
bash scripts/profile-render.sh 12345 12346 /tmp/yinz-notcurses-profile-before 30
```

The script creates a new directory and records both processes concurrently.
Each profile includes all threads and context-switch markers. It also saves
revision metadata, profiler logs, and one resource sample per second.
`resources.txt` contains PID, cumulative CPU time, lifetime-average CPU percentage,
and RSS in KiB. Linux samples also include the process's lifetime peak RSS
(`VmHWM`). Use fresh processes when comparing peak RSS between runs.

Open each capture separately:

```sh
samply load /tmp/yinz-notcurses-profile-before/server.json.gz
samply load /tmp/yinz-notcurses-profile-before/client.json.gz
```

Compare server parsing, grid rendering, output serialization, and socket delivery
with client terminal writes. Context-switch markers help separate CPU work from
time blocked on I/O. Profiled runs are separate from timing comparisons.

If `samply` cannot attach, inspect `server.log` and `client.log`. On Linux,
profiling permissions depend on `kernel.perf_event_paranoid` and system policy.
The script does not change those settings.

Existing `output pressure:` logs identify slow socket delivery, client enqueue,
terminal write/flush, and queue overflow. Capture them promptly from
`${TMPDIR:-/tmp}/yinz-$(id -u)/yinz-log/yinz.log`, which is truncated at 100 KB.
These logs report slow events, not continuous backlog measurements.

No render pacing or IPC behavior changes are included in this benchmark setup.
