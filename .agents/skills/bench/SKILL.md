---
name: bench
description: Run zellij's render-throughput benchmarks (cargo x bench) and compare against a saved baseline. Use when the user wants to benchmark, measure render performance, or check for perf regressions in the render path.
---

`cargo x bench` runs `zellij-server/benches/multi_pane_render.rs`: N terminal panes tiled over
one surface, all fed synthetic pty bytes at once, measured as three criterion groups —
`ingest` (vte parse only), `frame` (parse + diff + serialize, the headline number) and
`render_only` (diff + serialize with ingest untimed). Five workloads × pane counts 1–16.

Benchmarks are **not** run by CI and are not part of `cargo x test` (the `[[bench]]` target sets
`test = false`). Run them by hand around a change to the render path:

```sh
cargo x bench -- --save-baseline main     # before
cargo x bench -- --baseline main          # after; criterion prints the deltas
cargo x bench -- frame/plain_text         # a bare filter narrows the run
```

The server comparison stays strict. Hocus uses Criterion's lenient comparison because older
branches have no hocus benchmark baseline; it still measures those new cases without a delta.

First build is slow — `[profile.bench]` optimizes the whole server. Timings are only comparable
within one machine, and the workloads are seeded so input never drifts between runs.
