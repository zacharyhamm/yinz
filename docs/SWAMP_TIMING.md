# Swamp workflow timings

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
