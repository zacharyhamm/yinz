# Swamp workflow timings

This historical record includes the former Lua runtime. Current plugins use [Deno/Wasm](DENO_PLUGINS.md); see [migration](LUA_REMOVAL.md).

Set `ZELLIJ_SWAMP_TIMING=1` before starting the diagnostic Zellij server and its managed Swamp child. Diagnostics are disabled by default. Existing servers retain their startup environment.

Run the disposable acceptance scenario with the built binary:

```sh
python3 extensions/tests/smoke.py --timing-only --binary target/debug/zellij
```

This scenario validates the existing workflows, measures managed startup separately, then makes three diff-and-return trips on one warm server. It uses the existing answer and approval paths. It saves workflow history and logs through Swamp commands. The fixture directory is private and remains available after cleanup.

`swamp_timing` records use these scopes:

| Layer | Measurements | Correlation |
| --- | --- | --- |
| TypeScript | Process execution, response parsing, observation request count and time, deliberate waiting | Method, model ID, work/key, attempt, returned request/run IDs |
| Lua | Shared invocation, including awaited host operations and failure cleanup | Method, work/key, interpreter attempt, returned request/run IDs |
| Transport | Prewarm, dispatch lock, queue, connection, send, first response, run acknowledgement, terminal response, disconnect, total | Request ID, attempt, run ID when known |

TypeScript process time starts at bridge entry. `parseMs` measures parsing separately. Observation `requestMs` includes all polls, including the final output request. `waitMs` measures deliberate waits, including scheduler delay. Neither value changes the 500 ms polling interval.

Transport queue, first response, acknowledgement, terminal, and total times are cumulative from enqueue preparation, after dispatch lock acquisition. Prewarm, dispatch lock, connection, and send measure their own phases. A connection record exists only when a socket is opened. Reconnects retain request identity and receive a new diagnostic attempt. Do not add cumulative milestones together.

Individual invocation and transport outcomes describe that call, not the durable operation state. An observation summary records the observed operation outcome.

Lua milliseconds come from the host monotonic clock. Its epoch belongs to the interpreter. Compare differences, not absolute readings across processes.

The managed child writes `stdout.log` and `stderr.log` under `/tmp/zellij-swamp-<uid>/<checkout-path-sha256>/`. Each stream holds at most 10 MiB and one 10 MiB `.log.1` backup. Directories use mode 0700 and capture files use 0600. Stdout capture begins after the startup announcement. The reader retains any bytes already buffered after that announcement. Capture errors are logged and the streams continue to drain. After child exit, each capture task has up to one second to drain final bytes.

Bridge timing records select correlation fields explicitly. They exclude credentials, request payloads, prompts, command output, and error text. Raw Swamp captures can contain application data. Keep those private. No timing fields enter durable operation records.

The managed child sets `OTEL_TRACES_EXPORTER=console` and retains `--no-telemetry`. The installed Swamp build emitted console spans with this combination. Its usage telemetry switch does not disable the console trace exporter. No collector or dependency was added.

## Baseline

The main checkout run `6b16b4d1-69ed-488b-a91e-73a4b47fcd76` took 35,100 ms. History and logs were retrieved with `swamp workflow history get` and `swamp workflow history logs`.

| Step | Baseline ms |
| --- | ---: |
| attach | 2,520 |
| sync | 2,568 |
| inspect | 2,400 |
| diff | 2,276 |
| diff-exit | 10,447 |
| diff-return | 14,512 |
| Other step/job/run time | 377 |
| Total | 35,100 |

These durations overlap child runs. In particular, `diff-return` waits for the next preparation workflow. Adding that child duration to the parent total would count it twice.

The old logs cannot separate all causes. They place the diff method's dispatch-to-record interval at 302 ms within its 2,276 ms step. The inspect method's first-to-last log interval is 730 ms within its 2,400 ms step. The remaining time is not established as bridge latency. Observation includes time spent viewing the diff, and continuation includes child work and polling.

## Verified disposable measurements

Measured on 2026-09-10, after the build and test load finished. The base Zellij revision was `98af61d40ecff54850477b5d86b223b7edde41dd`, with this instrumentation. The installed Swamp help reported version `20260206.200442.0-sha.` and its spans reported `service.version=dev`. This does not identify an exact Swamp source revision.

Cold managed startup took **5,081 ms**. This includes credential bootstrap, process startup, and readiness. It is outside the warm-run totals.

| Step | Run 1 ms | Run 2 ms | Run 3 ms |
| --- | ---: | ---: | ---: |
| attach | 77 | 81 | 86 |
| sync | 65 | 66 | 73 |
| inspect | 79 | 81 | 91 |
| diff | 109 | 115 | 123 |
| diff-exit | 646 | 650 | 653 |
| diff-return | 765 | 770 | 812 |
| Other step/job/run time | 36 | 30 | 34 |
| Total | **1,777** | **1,793** | **1,872** |

The median total was **1,793 ms**. These are the same diff-and-return workflow path as the baseline, in a small disposable repository. They are not measurements of the main checkout.

The following accounting uses only the parent diff run. Ten TypeScript process calls match ten Lua invocations by method and work/key in each repetition. Duplicate copies of run-log entries are removed by attempt and phase.

| Parent wall-time component | Run 1 ms | Run 2 ms | Run 3 ms |
| --- | ---: | ---: | ---: |
| Git inspection method execution, including its data writes | 48.7 | 47.6 | 48.8 |
| Lua host calls, including managed transport | 160.7 | 161.7 | 205.6 |
| Bridge/IPC outside Lua: TypeScript process time minus Lua time | 243.4 | 250.5 | 254.1 |
| Deliberate observation waits | 1,001.8 | 1,002.6 | 1,001.1 |
| Remaining method, Swamp, and persistence time | 322.4 | 330.6 | 362.4 |
| Total | **1,777.0** | **1,793.0** | **1,872.0** |

The remainder is an explicit residual, not a measured CPU cost. It includes resource writes, response parsing, method setup, workflow bookkeeping, reports, and uninstrumented work. Parsing all ten bridge responses took only 2.0–2.3 ms per run. Console spans separately locate roughly 250–290 ms per run outside `swamp.method.execute`.

Both observation loops made three requests: two status polls and the final output request. Each loop waited once for approximately 500 ms. The continuation's wait overlaps the next preparation workflow. That child's work is already inside the parent's wall time and is not added again. The viewer is dismissed as soon as the harness observes its prompt. Its command execution and prompt-detection delay overlap observation, so the table does not claim to measure Git rendering CPU time separately.

Transport phase samples from the saved Zellij logs support a narrower conclusion: dispatch lock, queue, and socket send did not explain seconds of latency. The largest observed values were 0.126 ms for dispatch-lock acquisition, 0.064 ms for queue wait, and 0.215 ms for send. Warm prewarm checks took 2.1–3.1 ms. The two observed socket establishments took 16.9 and 27.6 ms. Run acknowledgements arrived within 78.4 ms of enqueue preparation. These samples are inside the Lua/bridge measurements above, not extra time to add to them. No reconnect or transport error occurred in the measured replay. The existing reconnect and lost-acknowledgement tests cover those paths.

The harness measured 6,847, 6,583, and 6,772 ms from selecting Show diff through observing the next preparation question. Those totals include answering/resuming the previous workflow, launching the diff child, and CLI polling used to observe the result. They are a different boundary from the 1.8-second child-run totals. Human approval time before each selection and the time used to retrieve logs between repetitions are excluded.

Raw evidence remains private at `/tmp/zellij-workflow-acceptance-iwy3bv49/`. The directory contains `timing.json`, per-run `.history.json` and `.logs.json` files, copied Zellij logs, and `analysis-records.json`. Console spans remain in `/tmp/zellij-swamp-1000/2f708c3c7e9dfdbdc722b7bc99c82ab3d50b4d997a27d2eedaccef05324562b0/`.

| Repetition | Diff run ID |
| --- | --- |
| 1 | `088bf1dd-5800-4014-a75d-0f8ad0d5fd2f` |
| 2 | `f7d7c842-f675-42e6-932b-25ad488af553` |
| 3 | `a430b2f1-7db1-490b-9758-5229aa55f007` |

## Interpretation and next change

Do not simplify the workflow or change its locks based on these measurements. The fixed waits explain about one second in the small checkout, not the original 35.1 seconds. The measured transport waits are small.

The baseline continuation's child, `390d88e3-620f-45ae-bde5-cab65b7c7efb`, spent 3,411 ms attaching, 6,026 ms syncing, 2,378 ms inspecting, and 148 ms opening its question. These 11,963 ms overlap the parent's 14,512 ms continuation step. Treating continuation as pure IPC overhead would misidentify most of that time.

Three later read-only identity probes to the baseline's existing Zellij session took 24.2, 24.3, and 26.0 ms. They did not restart the session or replay its workflow. They show that a simple bridge call was fast at measurement time, but do not reconstruct its earlier load.

The smallest justified next action is to enable these diagnostics when the main session and managed service are next started normally. Then capture a user-initiated slow run. Compare step duration against method-entry, bridge-process, Lua, and console-span durations. The baseline has roughly two seconds outside the logged bodies of several routine methods. That gap is the first target. Its cause remains unproven: the disposable checkout differs in repository state, workload, and the already-running service's lifetime. No production replay, active-server restart, or workflow redesign was performed here.

## Verification

- TypeScript: nine bridge, continuation, Zellij, and Git workflow tests passed.
- Rust: successful/failed correlated timings, disabled output, capture privacy/rotation, and awaited Lua timing passed.
- Existing transport tests passed for multiplexing, reconnect attachment, and read-only retry after lost acknowledgement.
- The final binary passed `extensions/tests/managed_server.ts` with diagnostics enabled, covering startup races, private credentials, active stop/restart refusal, crash reconciliation, and run-ID cancellation.
- The disposable scenario validated workflows and completed all three repetitions through the existing approval paths. It parsed saved timing records and verified zero per-operation Swamp CLI launches from Zellij or Swamp.
- Final `cargo x ci check` passed: format, lint, all 7,896 selected unit tests, and all 383 integration tests.

The gate exposed an archive test that required five total timeline entries. An asynchronous summary could legitimately add a sixth entry. The test now counts the three controlled message entries, preserving its duplicate-message check without depending on summary timing.

## Retained-history transition probe

The transition probe runs three fix-agent cycles. Each cycle saves a large failing-command log, dispatches a stub helper, and checks that helper exit creates a fresh question run. It sends the measured answers through the terminal buttons and records the arrival of the first terminal bytes that contain each question. The helper records prompt delivery and exit on the same monotonic clock. Its separate startup measurement covers stub initialization after Python imports. Cold managed-server startup is also recorded separately.

Use the same Swamp and Zellij binaries for both fixtures. The probe pins both executables. A binary under a `release` directory uses the release wasm plugins.

```sh
cargo x build --release
PATH=/path/to/patched-swamp/bin:$PATH python3 extensions/tests/smoke.py \
  --transition-timing-only --binary target/release/zellij
PATH=/path/to/patched-swamp/bin:$PATH python3 extensions/tests/smoke.py \
  --transition-timing-only --binary target/release/zellij \
  --history-runs 500 --swamp-source /path/to/swamp
python3 extensions/tests/timing_summary.py /tmp/zellij-workflow-acceptance-EMPTY \
  /tmp/zellij-workflow-acceptance-HISTORY > /tmp/warm-transitions.json
```

The history fixture contains 500 retained runs, 1,500 automatic definitions, and 1,500 method-output metadata records before the first measured cycle. Each measured cycle then retains real failure output and report bodies. The seeder uses Swamp's persistence adapters and accepts only disposable acceptance directories. It does not change the live repository.

The Swamp changes are in `extensions/tests/swamp-warm-transitions.patch`, based on Swamp revision `7b11196336e5f67c5f3a81f7938dedede26b406c`. Apply the existing `swamp-serve-resume.patch` first. Build the patched checkout with `deno task compile --output /path/to/patched-swamp/bin/swamp`.

The trace separates context construction, step dependencies, definition lookup, validation, expressions, evaluated/output persistence, locks, methods, reports, and flushes. Initial context construction precedes run-ID allocation. Resume context records include the existing run ID. The view records refresh, question rendering, and the delay from a ready snapshot to rendering. Nested method and expression durations overlap their child phases. Do not add them together.

The measured fixes remove repeated definition parsing, full-history reads for complete run IDs, and artifact-body reads before metadata filtering. Modern expressions use the existing lightweight context. Legacy `model` and `file` expressions retain full context construction, including references in stored definitions and bare assertions. Model and workflow identity hints are checked against current file metadata, and selected definitions are read again. This also avoids reparsing every workflow during history authorization when the supplied identifier is a run ID. Authorization still resolves against the current repository.

Views discover retained runs on attachment or recovery, then read the durable run registry and current run states. They share existing dispatch streams, receive notifications in manual and agent-exit views, and coalesce changes on the next event-loop turn. A timer does not queue another full snapshot while one is pending. Evidence reports use saved model-name provenance, with the existing lookup for legacy resources.

Each fixture retains `transitions.json`, exported workflow histories and logs, the terminal recording, and Zellij logs. The summary includes binary hashes, the report source hash, repeated transition measurements, and phase statistics. Raw evidence stays in the private fixture directory.

## Verified warm transitions, 2026-09-12

All 18 optimized transition measurements were below one second: **389–865 ms**, including the fixture's Git operations and stub execution. No method duration was subtracted to meet the target. Cold managed startup remained separate at 6.58–6.68 seconds for the optimized fixtures.

The following values are medians of three cycles, in milliseconds. “Retained” starts with the history described above. Even the initially empty fixture retains the outputs and reports from each preceding cycle.

| Transition | Empty baseline | Empty optimized | Retained baseline | Retained optimized |
| --- | ---: | ---: | ---: | ---: |
| Answer to next rendered question | 3,713 | 591 | 13,905 | 790 |
| Answer to helper receiving its prompt | 6,442 | 618 | 26,638 | 806 |
| Helper exit to next rendered question | 6,266 | 478 | 23,039 | 752 |

The baseline reproduced the growing delay. Its third retained-history cycle took 52,749 ms to deliver the helper prompt and 64,715 ms from helper exit to the next question. The corresponding optimized values were 865 and 813 ms.

Both variants used the same release Zellij binary and current evidence report. Baseline Swamp contained the resume compatibility patch and identical phase instrumentation, without the optimization patch. Thus this comparison isolates the Swamp changes with the improved Zellij event delivery held constant. The final optimized fixtures ran serially after builds and baseline measurements finished. These are disposable fixture results, not live-session measurements or a latency guarantee for real agents.

The retained-history phase medians locate the removed cost:

| Phase | Baseline ms | Optimized ms |
| --- | ---: | ---: |
| Step definition lookup | 2,690.3 | 48.8 |
| Step dependency construction | 1.3 | 0.6 |
| View snapshot refresh | 2,024.7 | 77.9 |
| Workflow reports | 599.5 | 119.0 |
| Ready question to render | 4.7 | 4.7 |

The final workflow identity optimization addressed repeated authorization misses: a history request tried its run ID as a workflow name and ID, reparsing every workflow on both misses. A separate warm repository probe reduced that lookup from 14–21 ms to 0.35–0.43 ms. The hints skip only unchanged, unrelated definitions; selection and authorization still read current data.

Method measurements remain separate from orchestration phases. In the optimized retained fixture, medians were 65.1 ms for helper spawning, 31.7 ms for prompt delivery, 53.7 ms for Git inspection, 68.8 ms for commit, and 106.1 ms for rebase. These include each method's own result writes. Stub initialization after imports took 5.16–5.43 ms. Continuation methods include their child workflows and observation waits, so their durations are not pure overhead.

Question delivery was independent of report completion. The fresh exit questions arrived 703–759 ms before the prior failed-test run's reports completed in the retained fixture. The question-ready-to-render delay remained below 10 ms. This avoids treating report completion as the user-visible transition boundary.

[SWAMP_WARM_TRANSITIONS.json](SWAMP_WARM_TRANSITIONS.json) retains every sample, per-phase and method statistics, report-completion offsets, fixture paths, and executable/report hashes. No prompts or command output are included.

The final checks passed 1,058 affected Swamp tests, Swamp formatting and lint, 13 extension tests, compatibility and managed-service lifecycle probes, and the release extension acceptance scenarios. The Zellij gate passed format, lint, 7,992 unit tests, and 402 integration tests. The existing diff timing and question-latency probes also passed. Regression tests count definition/body reads, context construction, discovery, stream attachment and queued refreshes instead of asserting elapsed time.

The release acceptance run covers duplicate and interrupted answers, resumed runs, concurrent executions, stale approval evidence, large failure logs, exact saved failure delivery, uncertain dispatch, server crash/adoption, and fresh question identities. The gate also exposed obsolete agent-picker labels and a test that raced asynchronous resizes; those assertions now use the current labels and a settled viewport, with the expand/restore behavior preserved.

The local Swamp source checkout contains the changes, and the portable patches reproduce it. Installed binaries and the live managed service were not changed during these measurements. Activate the matching builds and extensions at an idle or suspended boundary, then verify a normal live transition.
