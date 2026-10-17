# Original runtime references

These compressed JSON files contain observations from the original Wasm and Deno plugins.
The migration compared each original plugin with its Scheme port before removing the old runtimes.
The fixtures preserve that evidence and continue to test the real Steel implementation.

The validation ran on 2026-10-14 with this worktree's own Wasm artifacts and native compilation worker.
It used `rustc 1.99.0 (b940084d7 2026-09-28)` on `x86_64-unknown-linux-gnu`.
The worktree started at commit `c3a073b34144ee786b6f7cbcca5dc698439cd811`.
Its migration changes were uncommitted during capture.

The migration recorded these completed validation runs:

| Run | Result | Migration log filename |
|---|---|---|
| Live original-runtime comparisons with engine and builtin checks | 285 passed | `yinz-steel-full-actual-parity.log` |
| Complete live status, tab, and compact bar comparisons | 27 passed | `yinz-steel-bar-trace-live.log` |
| Original-runtime capture | 121 passed, producing 204 fixtures | `yinz-steel-reference-full-capture.log` |
| Reference replay with migrated host and installed-test checks | 247 passed, including all 121 comparisons and 3 trace integrity checks | `yinz-steel-reference-host-migration-verified.log` |

The runs overlap and their test counts are not additive.
The log filenames identify the migration's temporary validation logs, which are not part of this directory.
The 204 captured fixtures contained 155,611,723 bytes of JSON, stored in 3,384,061 compressed bytes.

The configuration-palette fixture's navigation and combined scalar form were adapted directly
from main's TypeScript implementation and SDK at `539b78afe`. Its `adaptation` metadata records
this intentional change; the original source fingerprint, host snapshots, configuration write,
and completion remain intact. These changed display expectations were not captured from Steel.

The Agents fixtures adapt the original render streams for the pane readability change.
Their `adaptation` metadata identifies this change. Text style levels changed, and
opaque text plus empty rows now apply the semantic theme background across the viewport.
The original text, coordinates, selection markers, lifecycle inputs, host calls, replies, and source fingerprints remain intact.
Independent style transformations produced these expectations. The current renderer was checked against them.

Each fixture records its original source SHA-256, engine, plugin, configuration, and ordered lifecycle steps.
The Wasm fingerprint covers the original artifact bytes.
The Deno fingerprint covers the embedded source tree and SDK.
Bar fingerprints cover both original implementations.
Each filename identifies a test scenario, plugin, and instance number within that scenario.
Keybinding fixtures from configuration maps are sorted before either guest receives them because the maps have no stable iteration order.
Tests with explicitly ordered custom bindings keep that order.

Steps retain inputs, update decisions, typed host commands, and render streams.
Wasm traces also retain decoded host replies, including failures.
Deno and bar traces retain their deterministic host settings and command results.
Bar traces retain status-tip cache changes and controlled random choices.
The regression for the original plugin-manager render trap also retains that failure.

Replay runs the Scheme plugin and the test's host services normally.
It also executes recorded Wasm commands against the independent reference backend and checks each reply.
The tests compare commands, update decisions, rendered terminal cells and styles, and database and filesystem effects.
Replay fails if the input sequence changes or a fixture has unused steps.
Only temporary fixture-root paths are replaced with `<fixture-root>`.
Existing normalization rules still apply, including unordered subscriptions and asynchronous request identifiers.

## Run the comparisons

The tests always replay the checked reference fixtures.
Run this command from the repository root:

```sh
cargo x test -p yinz-plugin-host -E 'test(parity) | test(plugin_tests::status_bar) | test(plugin_tests::tab_bar) | test(plugin_tests::compact_bar) | test(reference_trace::reference_trace_)' -j 16
```

Set `YINZ_REFERENCE_TRACE_DIR` to read a separate fixture directory for experiments.
The original runtime sources remain available in repository history.
There is no capture mode in the current tests.
Do not replace reference fixtures with observations from the Scheme implementation.

Inspect a fixture with `gzip -dc path/to/fixture.json.gz | python3 -m json.tool`.
Compression changes storage only. It preserves the full JSON trace and its provenance.
