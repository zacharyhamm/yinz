# zellij-server test audit

Companion to `docs/ZELLIJ-SERVER-TESTS.md`. Two conventions:

- **A ticked box in `ZELLIJ-SERVER-TESTS.md` means the test has been audited**, not that it is good.
- **An entry in *this* file means the test needs work.** A test judged sound is named in the
  summary below and gets no item. So this file only ever grows work, and shrinks as it is done.

Each item says what would *still pass* if the behaviour under test broke, because that — not
coverage — is what makes a test worth its runtime. Findings are marked **[mutation]** when they
were demonstrated by breaking the production code and watching the test stay green, or
**[inspection]** when reading settles it.

## Pass 1 — items 1-30 (`ZELLIJ-SERVER-TESTS.md` lines 5-34)

Audited 2026-09-08. 30 tests: `command_output_tests` (2), `background_jobs::tests` (5),
`web_request_stream_tests` (17), `instruction_context_tests::{background_job,plugin}` (6).
All 30 pass at HEAD (42 tests in 1.08s, `cargo x test -p zellij-server`).

**16 sound, 14 need work, plus 3 gaps that belong to no single test.**

**All 25 items below are done (2026-09-08)**; each carries a *Done* note saying what landed and
which mutation now kills it. Five tests were renamed in the process, and `ZELLIJ-SERVER-TESTS.md`
follows: `scan_session_list_finds_resurrectable_from_orphan_layout`,
`redirect_status_and_verb_decide_the_rewrite`, `scrub_redirect_headers_drops_every_credential_header`,
`finish_stream_supersede_flushes_pending_bytes_then_terminates` and (pass 2)
`locked_escape_is_cancelled_by_mode_configuration_and_passthrough_changes`. Every mutation named
in a *Done* note was re-applied after the fix and the named test went red; all were reverted.

Sound, no action: `unbounded_command_output_is_unchanged` (its sibling's `tokio::time::timeout`
would be worth copying, so a hang fails fast rather than waiting for nextest's kill),
`scan_session_list_returns_empty_when_no_peers`, `scan_session_list_reads_published_checkpoints`,
`an_http_redirect_to_a_file_url_is_rejected`, `a_safe_http_redirect_is_followed`,
`a_burst_of_frames_arrives_as_one_event_after_the_coalescing_window`,
`cancellation_flushes_bytes_read_before_the_coalescing_deadline`,
`non_2xx_response_is_buffered_whole_with_no_chunks`,
`terminator_is_emitted_exactly_once_per_stream`,
`completed_stream_terminates_without_a_second_result`,
`cancelling_an_unknown_stream_is_a_no_op`,
`stale_finish_does_not_cancel_replacement_stream` (its second half duplicates the tail of
`reusing_a_stream_id_supersedes_the_old_stream`; harmless), and
`{background_job,plugin}::{every_instruction_resolves,every_context_is_reachable}`.

Three deserve a note for being unusually good, since the rest of this file is criticism:
`a_burst_of_frames_arrives_as_one_event_after_the_coalescing_window` pins the coalescing window
with paused time and an absolute deadline, so it is exact without being timing-dependent;
`terminator_is_emitted_exactly_once_per_stream` drives the real cancel/finish race and checks the
plugin's correlation context survives it; and `an_http_redirect_to_a_file_url_is_rejected` runs a
real redirect over a real socket to prove the SSRF guard fires mid-chain.

### Cross-cutting gaps

- [x] **`scan_session_list`'s own contribution is never reached.** All five
      `background_jobs::tests::scan_session_list_*` pass `"me"` as the current session and never
      create a session by that name, so the `if name == current_session_name` branch at
      `background_jobs.rs:1364-1369` — `populate_plugin_list` and `available_layouts`, the only
      thing `scan_session_list` adds over `read_live_session_states` +
      `find_resurrectable_sessions` — is dead to the suite. **[mutation]** Gating that branch on
      `false` leaves all five green. *Fix:* one test that names the current session, creates its
      socket and metadata, and asserts the returned `SessionInfo` carries the plugin list and
      layouts that were passed in — and that no *other* session's entry does.
      *Done:* `scan_session_list_enriches_only_the_current_session` names the current session,
      publishes it and a peer, and asserts the plugin list and layouts land on the current
      entry and not the peer's. Killed by gating the branch on `false`.

- [x] **No web test uses the production HTTP client.** Every redirect test builds
      `HttpClient::new()`. The client that ships is built at `background_jobs.rs:292-299`, and its
      `.redirect_policy(RedirectPolicy::None)` is the whole reason `send_web_request` gets to vet
      each hop through `web_url` — change it to `Follow` and curl chases redirects itself,
      bypassing the `file://` guard that `an_http_redirect_to_a_file_url_is_rejected` exists to
      protect. Nothing would fail. **[inspection]** *Fix:* extract the builder into a
      `fn web_http_client() -> Option<HttpClient>` used by both `background_jobs_main` and the
      tests, so the redirect tests exercise the shipped configuration.
      *Done:* extracted `web_http_client()`; every test client is now built by it.

- [x] **Nothing drives `background_jobs_main`'s stream dispatch.** The stream tests call
      `finish_stream` directly and hand-assemble `active_streams`. The `WebRequest` (`:517-611`),
      `CancelWebRequest` (`:612-621`) and `WebRequestStreamFinished` (`:622-630`) arms, and the
      `next_stream_generation` counter that makes the stale-finish guard work at all, have no
      test. **[mutation]** Deleting the supersede `finish_stream` call at `:572-578` — the one
      thing stopping a reused stream id from orphaning a running task — leaves the whole suite
      green. *Fix:* a test that runs `background_jobs_main` on a bus and feeds it
      `WebRequest{stream_id} → WebRequest{same stream_id} → CancelWebRequest`, asserting the
      terminator sequence and that the two generations differ.
      *Done:* `the_web_request_arm_supersedes_and_cancels_through_the_bus` runs
      `background_jobs_main` on a bus and feeds it `WebRequest → WebRequest (same id) →
      CancelWebRequest`, then a completing stream whose own `WebRequestStreamFinished` comes
      back over the bus. Killed by deleting the supersede call.

### `command_output_tests`

- [x] **`bounded_command_output_is_truncated_and_drained` cannot tell a prefix from a suffix.**
      It writes 100_000 identical `b'x'` bytes, so `assert_eq!(output.len(), 40)` plus
      `ends_with(OUTPUT_TRUNCATED)` holds for any policy that keeps *some* 28 bytes.
      **[mutation]** Rewriting `read_command_stream` (`:52-82`) to keep the **last** `limit` bytes
      instead of the first leaves it green — yet "the head of the output, then `[truncated]`" is
      the contract every plugin reading `RunCommandResult` relies on. *Fix:* write a
      distinguishable payload (e.g. `(0..100_000).map(|i| (i % 251) as u8)`) and assert the kept
      prefix equals the first `limit - OUTPUT_TRUNCATED.len()` bytes of it.
      *Done:* `(i % 251)` payload, kept prefix asserted byte-for-byte. Killed by a
      keep-the-tail reader.
- [x] Same test, second gap: `limit < OUTPUT_TRUNCATED.len()` is never exercised, though
      `read_command_stream` carries explicit `saturating_sub` / `.min(limit)` arithmetic for it
      (`:78-79`). *Fix:* add `Some(5)` and `Some(0)` cases asserting `output.len() == limit`.
      **[inspection]**
      *Done:* `bounded_command_output_honours_a_limit_smaller_than_the_marker` covers
      `Some(5)` and `Some(0)`.
- [x] Same test, third gap: the "drained" half of the name is real but only asserted by accident —
      a reader that stopped early would drop the `DuplexStream` and surface as a `JoinError` from
      `write.await.unwrap()`, not as a named failure. *Fix:* have the writer task return its byte
      count and assert it, so the failure says "the pipe was not drained". **[inspection]**
      *Done:* the writer returns its byte count and the shared `read_bounded` helper asserts
      it with a "not drained" message.

### `background_jobs::tests` (session scanning)

- [x] **Three of the five tests exercise a legacy compatibility path, not the one production
      writes.** `write_metadata` and `write_layout` drop bare `session-metadata.kdl` /
      `session-layout.kdl` into the folder with no `.checkpoint.lock` and no `current` symlink, so
      `SessionCheckpoint::read` takes its "legacy reader racing the first publication" branch
      (`zellij-runtime/src/session_checkpoint.rs:27-40`) and reads the flat files. Real sessions
      always go through `write_session_state_to_disk` → `write_checkpoint`, which publishes a
      `current` symlink. So `scan_session_list_finds_peer_from_socket_and_metadata`,
      `scan_session_list_finds_resurrectable_from_orphan_metadata` and
      `scan_session_list_separates_live_from_resurrectable` are all testing the fallback.
      **[inspection]** *Fix:* build the fixtures with `write_checkpoint`, as
      `scan_session_list_reads_published_checkpoints` already does, and keep one flat-file test
      that says in its name that it covers the legacy layout.
      *Done:* fixtures go through `write_checkpoint` (`publish`);
      `scan_session_list_reads_the_legacy_flat_files` is the one flat-file test and says so.
- [x] **`scan_session_list_finds_resurrectable_from_orphan_metadata` is named for the wrong file.**
      It calls `write_layout` and writes no metadata at all, and `find_resurrectable_sessions`
      (`:1391-1441`) reads no metadata either — it keys on the layout file's existence and ctime.
      **[inspection]** *Fix:* rename to `..._from_orphan_layout`, and add the case the current name
      implies: a folder with metadata but *no* layout, which `:1417-1419` deliberately drops
      because it cannot be resurrected. That branch is untested.
      *Done:* renamed `..._from_orphan_layout`;
      `scan_session_list_drops_a_folder_with_metadata_but_no_layout` pins the `NotFound` drop.
- [x] Same test: it asserts `resurrectable.len() == 1` and ignores the returned `Duration`, which
      is the "dead N ago" figure the session manager shows. A regression returning
      `Duration::default()` for every dead session passes. *Fix:* assert the duration is small but
      derived (e.g. `< Duration::from_secs(60)`), and add a case where the layout's ctime is
      unreadable to pin the `unwrap_or_default()` fallback. **[inspection]**
      *Done:* the duration is asserted `< 60s`. The `unwrap_or_default()` fallback needs a
      non-`NotFound` error from `metadata().created()`, which no portable fixture produces (a
      dangling symlink is `NotFound`, a directory succeeds, `chmod` fails earlier in
      `SessionCheckpoint::read`); it is documented on the production line instead of tested
      blind.
- [x] **`scan_session_list_separates_live_from_resurrectable` leaks three fds for no reason.**
      `std::mem::forget(_listener)` at `:1575` looks load-bearing but is not: dropping a
      `UnixListener` closes the fd without unlinking the socket file, and
      `read_live_session_states` only stats the dirent — it never connects. **[mutation]**
      Removing the `forget` leaves the test green. *Fix:* delete it, or bind the listeners into a
      `Vec` held for the test's lifetime if keeping them open was the intent.
      *Done:* the `forget` is gone; listeners live in a `Vec` for the test's lifetime.
- [x] Same test: "live" here means only *a socket file exists*, since nothing in the path connects
      to it. That is production's definition too, so the test is not wrong — but the name promises
      a liveness check it does not make, and a stale socket from a crashed server reads as live.
      *Fix:* a comment saying so, so the next reader does not assume liveness is verified.
      **[inspection]**
      *Done:* the doc comment says so.

### `web_request_stream_tests`

- [x] **`cross_origin_redirects_drop_sensitive_headers` never performs a redirect.** It calls the
      pure `scrub_redirect_headers` helper (`:944-951`) directly. The decision that actually
      protects a bearer token — `if url.origin() != next_url.origin()` at `:1005-1007` — has no
      test at all. **[mutation]** Gating that call on `false`, so credentials follow every redirect
      to any host, leaves every web test green. This is the most serious finding in the pass: the
      test's name describes a guarantee the suite does not check. *Fix:* two socket-backed tests
      built on `serve_redirect` — one redirecting to a *different* origin, asserting the second
      request carries no `Authorization`; one redirecting within the same origin, asserting it
      still does. The same-origin case matters as much: over-scrubbing silently breaks
      authenticated APIs that redirect.
      *Done:* `a_cross_origin_redirect_drops_credentials` redirects to a second listener (a
      different origin) and asserts the second request carries neither `Authorization` nor
      `Cookie`; `a_same_origin_redirect_keeps_credentials` asserts both survive a same-origin
      hop. Each is killed by the opposite mutation of the origin check.
- [x] Same test: it covers 2 of the 5 names `scrub_redirect_headers` removes (`cookie2`,
      `proxy-authorization` and `www-authenticate` are unlisted), so dropping one from the
      production list passes. *Fix:* assert against the full set. **[inspection]**
      *Done:* renamed `scrub_redirect_headers_drops_every_credential_header`, all five names
      in mixed case.
- [x] **`web_requests_accept_only_http_and_https_urls` misses the host check.** It tries three URLs
      and none is hostless. **[mutation]** Deleting `|| url.host_str().is_none()` from `web_url`
      (`:936`) leaves it green — yet that clause is what stops a hostless `http:` URL reaching
      curl. *Fix:* add the hostless case, plus one non-`file` foreign scheme (`ftp://`, `data:`)
      so the assertion is about the allowlist rather than about `file` specifically.
      *Done:* `ftp://`, `data:` and three hostless forms added. Note: the `url` crate never
      yields a parsed `http` URL without a host (`http:///x` parses as `http://x/`), so the
      hostless inputs are refused at parse time and the `host_str().is_none()` clause is
      unreachable for the allowed schemes; the test comment says so.
- [x] **`a_302_redirect_only_rewrites_post_to_get` tests one cell of a three-status matrix.**
      `serve_redirect` only ever answers 302, so of `status == 303 || (matches!(status, 301 | 302)
      && verb == HttpVerb::Post)` (`:1009`) only the 302+POST and 302+PUT cells are covered.
      **[mutation]** Narrowing production to `matches!(status, 302) && verb == HttpVerb::Post` —
      dropping 301 and 303 rewriting entirely — leaves it green. *Fix:* parameterise
      `serve_redirect` by status and cover 301+POST (rewrites), 303+PUT (rewrites — 303 rewrites
      *any* verb), and 307+POST (must **not** rewrite; production gets this right today by
      omission, which is exactly the kind of correctness that regresses unnoticed).
      *Done:* `redirect_status_and_verb_decide_the_rewrite` covers 301+POST, 302+POST,
      302+PUT, 303+PUT and 307+POST against a status-parameterised `serve_redirect`. Killed by
      narrowing to 302+POST.
- [x] Same test: every case passes `vec![]` as the body, so `body.clear()` at `:1011` — the half of
      the rewrite that stops a POST payload being replayed as a GET — is never observed. *Fix:*
      send a non-empty body and assert the redirected request carries no `Content-Length` and no
      body. **[inspection]**
      *Done:* every case sends `payload`; rewritten cases assert no `Content-Length` and an
      empty body, 307 asserts the body survived. Killed by dropping `body.clear()`.
- [x] **`MAX_WEB_REQUEST_REDIRECTS` has no test.** `serve_redirect` serves exactly one hop, so the
      10-redirect cap at `:230` and its `"exceeded {N} redirects"` error (`:998-1002`) are
      unexercised; a redirect loop is a hang, which is why the cap exists. *Fix:* a server that
      answers every request with a 302 back to itself, asserting the error. **[inspection]**
      *Done:* `a_redirect_loop_fails_at_the_redirect_cap` against a self-redirecting server,
      asserting the error and exactly `MAX + 1` requests.
- [x] **`buffered_web_bodies_stop_at_the_limit` tests only the far side of the boundary.** 5 bytes
      against a limit of 4 is over by one; `len == limit` — the case `read_bounded_web_body`'s
      `take(limit + 1)` / `> limit` pair (`:1052-1059`) exists to distinguish — is never tried.
      **[mutation]** Changing `> limit` to `>= limit`, which would reject every response of exactly
      the maximum size, leaves it green. *Fix:* assert that `limit` bytes succeed and return the
      whole body, alongside the existing over-limit case.
      *Done:* `buffered_web_bodies_of_exactly_the_limit_are_returned_whole`. Killed by `>=`.
- [x] Related, worth deciding rather than testing blind: `MAX_BUFFERED_WEB_RESPONSE_BYTES` is
      defined as `MAX_IPC_FRAME_LEN` exactly (`:231`), i.e. 64 MiB, while
      `zellij-ipc/src/ipc.rs:564` refuses to send a frame whose *encoded* length exceeds that. A
      body admitted at exactly the limit cannot fit alongside its status, headers and context if
      the event is ever framed. `screen/warp_ops.rs:75` handles the same tension by budgeting half
      a frame. *Fix:* either subtract an envelope allowance from the constant, or comment why the
      plugin path never frames these. **[inspection]**
      *Decided:* no allowance. `Event::WebRequestResult` reaches the plugin thread on an
      in-process channel and is never IPC-framed; the constant's comment says so and names
      `warp_ops.rs` as the place to copy if that changes.
- [x] **`streams_body_in_chunks_and_reports_finished_once` asserts `chunks.len() >= 2` for a case
      that yields exactly 3.** The server gaps 150ms between three parts against a 20ms coalescing
      window (`:2066`). **[mutation]** Widening the window to 200ms drops the count to 2 and the
      test still passes — a 10x coalescing regression, invisible. Tightened to `>= 3` it fails on
      the mutant and passes 3/3 runs at HEAD. *Fix:* `>= 3`, with a comment tying the number to the
      150ms gap versus the window, so the next reader knows why it is safe to be exact.
      *Done:* `>= 3`, with the 150ms-vs-20ms reasoning in a comment.
- [x] **`unreachable_host_reports_an_error_and_still_finishes` indexes `events[0]` without bounding
      `events`.** A regression emitting an extra `WebRequestResult`, or a chunk after the failure,
      passes. It also never asserts the 400's body carries the error text the plugin will display.
      *Fix:* assert `events.len() == 1` and check the body is non-empty. **[inspection]**
      *Done:* matches `events` as exactly one 400 result with a non-empty body.
- [x] **`truncated_non_2xx_response_finishes_with_an_error` has the same unbounded indexing, and
      never asserts the body.** That body is the interesting part: on a mid-body read failure
      `streamed_web_request` (`:1130-1137`) discards the partial bytes and substitutes the error
      string, a deliberate choice no test records. *Fix:* assert `events.len() == 1` and that the
      body is the error text, not `"short"`. **[inspection]**
      *Done:* exactly one 500 result, whose body is the finish report's error text and not
      `short`.
- [x] **`reusing_a_stream_id_supersedes_the_old_stream` re-implements the code it is named for.**
      Its comment says "what the WebRequest arm does before spawning a replacement", and the test
      body then does exactly that — so it tests the test. **[mutation]** Deleting the real call at
      `:572-578` leaves it green. Covered by the dispatch gap above; noted here because the comment
      reads as if the arm were under test. *Fix:* drive `background_jobs_main`, or retitle to say
      it covers `finish_stream`'s supersede semantics rather than the arm's use of them.
      *Done:* retitled `finish_stream_supersede_flushes_pending_bytes_then_terminates`,
      comment corrected; the arm is covered by the bus test above.

### `instruction_context_tests`

The three tests are generated by one `pin_context_to_instruction!` macro
(`unit/instruction_context_tests.rs:31-108`) and instantiated six times, so the finding below is
the macro's and applies to all six modules, not only the `background_job` and `plugin` pair in this
pass. `every_instruction_resolves` and `every_context_is_reachable` are sound and genuinely
valuable — they restore the exhaustiveness the hand-written matches used to give the compiler.

- [x] **`{background_job,plugin}::the_special_case_lists_are_current` never checks where a special
      case points.** For each name in `SPECIAL` it asserts the instruction variant still exists and
      that `Context::from_str` still fails — i.e. that a hand-written arm is still *needed*. It
      never calls `From<&Instruction> for Context`, so it cannot see that arm's target.
      **[mutation]** Repointing `CancelWebRequest | WebRequestStreamFinished` from
      `BackgroundJobContext::WebRequest` to `::Exit` (`background_jobs.rs:202-204`) leaves all
      three `background_job` tests green — and mislabelled error contexts are precisely what this
      module exists to prevent. The hand-written arms are the ones the compiler cannot check and
      the ones most likely to be wrong.
      *Done:* the macro takes `targets: [(name, sample instruction, expected context)]` and
      generates `special_cases_land_on_their_context` for all six modules. Killed by
      repointing the `WebRequest` arm to `Exit`.
- [x] Same tests, related: a name added to `SPECIAL` with no arm behind it also passes. It falls to
      the `other =>` catch-all, fails `from_str`, and degrades to `BackgroundJobContext::Exit` in
      release via the `debug_assert!(false)` at `background_jobs.rs:208-209`. *Fix for both:*
      extend the macro to a `special: [("CancelWebRequest", "WebRequest"), ...]` shape carrying
      each special case's expected context, and generate a fourth test that constructs the
      instruction and asserts the conversion lands there. Constructing arbitrary variants is the
      obstacle; a small per-enum `fn sample(name: &str) -> Option<Instruction>` covering only the
      `SPECIAL` names keeps it to a handful of lines, and is itself kept honest by
      `the_special_case_lists_are_current`. **[inspection]**
      *Done:* the same test fails for a `SPECIAL` name with no target, and for a target whose
      sample is not the variant it claims.

## Method

Every test in the pass was read against the production code it covers. Where a claim was stronger
demonstrated than argued, the production code was broken and the suite re-run; ten such mutations
were applied across two batches and **all ten survived**, each confirming the finding it was aimed
at. All mutations were reverted and the tree verified clean — no `.rs` file is modified by this
audit.

## Pass 2 — items 31-80 (`ZELLIJ-SERVER-TESTS.md` lines 35-84)

Audited 2026-09-08. 50 tests: `instruction_context_tests::{pty,pty_write,screen,server}` (12),
`locked_escape::tests` (9), `logging_pipe::logging_pipe_test` (7), `nested_guest::tests` (7),
`notifications::notifications_tests` (7), `os_input_output::os_input_output_tests` (8).
All 50 pass at HEAD (76 tests in 0.34s narrowed, `cargo x test -p zellij-server`).

**29 sound, 21 need work, plus 5 gaps that belong to no single test.**

**All 26 items are done (2026-09-08)**, each with a *Done* note; every mutation named in one was
re-applied after the fix, went red, and was reverted. Four tests were added
(`a_line_ahead_of_a_split_code_point_waits_for_it`,
`a_line_that_wraps_the_ring_buffer_is_still_logged_whole`, `removing_a_dormant_guest_forgets_it`,
`the_wait_does_not_return_while_a_writer_is_still_blocked_on_the_socket`) and four vacuous ones
removed (`{pty,pty_write}::{special_cases_land_on_their_context,the_special_case_lists_are_current}`);
`ZELLIJ-SERVER-TESTS.md` follows.

**The batch boundary cuts two groups, and the cut means nothing.** `client_writer_flush` has six
tests; the alphabetical slice takes four and leaves `shutting_a_client_down_unblocks_the_router_reading_it`
and `waiting_gives_up_rather_than_holding_the_process_open` for pass 3 — the second of those is
where the lower bound on `wait_until_drained`'s deadline would live, so the
`draining_is_reported_once_every_writer_is_done` reading below is provisional until it is read. The
top-level firecracker family is cut the same way: two of its seven members are in this batch, five
are not. Neither split follows a seam in the code.

Sound, no action: `{pty,pty_write,screen,server}::every_instruction_resolves`,
`{pty,pty_write,screen,server}::every_context_is_reachable`,
`locked_escape_{routing_respects_custom_bindings_and_explicit_passthrough,
window_follows_the_configured_timeout,deadline_and_modifiers_are_exact,
completion_writes_raw_ctrl_g_for_both_encodings,resize_does_not_resolve_or_extend_the_prefix}`,
`nested_guest::{announced_guest_gets_pinged_and_pong_keeps_it_alive,
suspended_guest_revives_on_reannounce_however_long_later,reannounce_of_live_guest_is_a_refresh,
pong_refreshes_the_expiry_deadline,pong_from_unannounced_pane_is_ignored}`,
`notifications::{a_kitty_host_is_detected_from_its_window_id,
a_missing_title_or_body_is_not_padded_with_a_separator,
an_explicitly_configured_protocol_overrides_detection,
an_unrecognized_host_falls_back_to_the_legacy_protocol,
detection_is_used_when_the_protocol_is_configured_to_auto,
each_protocol_renders_its_own_escape_sequence}`,
`a_firecracker_pane_adopts_the_worktree_the_provider_names`,
`a_refused_rename_keeps_the_old_firecracker_session_key`, and
`client_writer_flush::{a_lifted_deadline_no_longer_bounds_the_read,
a_silent_peer_reads_as_idle_once_the_deadline_passes,draining_is_reported_once_every_writer_is_done}`.

Four deserve a note for being unusually good. `locked_escape_deadline_and_modifiers_are_exact`
checks `LockedEscape::completes` at `deadline - 1ns`, `deadline` and `deadline + 1ns` — both sides
of the boundary, in a suite where one-sided limits are the recurring failure. `a_silent_peer_reads_as_idle_once_the_deadline_passes`
asserts the read both waited *and* gave up, and says in a comment that the upper bound is
deliberately generous. `a_firecracker_pane_adopts_the_worktree_the_provider_names` checks the rerun
command as well as the resolved one, with a comment explaining why that is not redundant. And
`locked_escape_window_follows_the_configured_timeout` derives the expected deadline from a
before/after `Instant` bracket rather than sleeping.

`logging_pipe` is the outlier in the other direction: **all seven of its tests need work**, for one
reason — see the group item below.

### Cross-cutting gaps

- [x] **Nothing in the `logging_pipe` suite asserts that anything is logged.** All seven tests
      assert on `pipe.buffer` alone; `LoggingPipe::log_message` (`logging_pipe.rs:26-34`) — the
      whole purpose of the type, which is what puts a plugin's stderr into the zellij log — is
      never observed. **[mutation]** Deleting both `self.log_message(msg)` calls from `flush`
      (`:74`, `:78`) while leaving the `consumed_bytes` arithmetic intact leaves all seven green:
      a pipe that silently swallows every plugin log line passes its own test suite. *Fix:* give
      `LoggingPipe` a test seam for the sink — the smallest is an `#[cfg(test)] logged: Vec<String>`
      pushed to alongside the `debug!` — and assert the exact lines each test expects, which also
      fixes the six items below at a stroke.
      *Done:* `#[cfg(test)] logged: RefCell<Vec<String>>` pushed to in `log_message`; all
      seven tests assert the exact lines (including that each bare `\n` logs a blank line,
      which is the pipe's real behaviour). Killed by dropping the `log_message` calls.

- [x] **The `locked_escape` expiry timer has no test.** `prepare_input` spawns a tokio task at
      `locked_escape.rs:187-198` that sleeps to the deadline and sends
      `ServerInstruction::ResolveLockedEscape`. It is the only thing that ever unlocks an abandoned
      prefix on the clock — without it a user who presses Ctrl+g and walks away stays visibly
      locked until the next keystroke. **[mutation]** Deleting the whole `spawn` block leaves all
      nine `locked_escape` tests green, and there is no coverage anywhere else: nothing under
      `zellij-integration-tests/` or `src/tests/` mentions locked escapes. *Fix:* one test that
      configures a short timeout, calls `prepare_input`, and waits on the `to_server` receiver for
      a `ResolveLockedEscape(client, token)` whose token matches the pending escape.
      *Done:* `locked_escape_expiry_timer_reports_to_the_server_with_its_token` with a 20ms
      window. Killed by a timer that never sends.

- [x] **Two of the six sites that cancel a pending escape are never driven.**
      `locked_escape_is_cancelled_by_mode_configuration_and_disconnect_changes` covers the four
      `SessionMetaData` helpers (`lib.rs:501`, `:541`, `:559`, `:572`). The other two are the
      `KeyPassthroughChanged` server arm (`lib.rs:2261`) and the client-disconnect path
      (`route.rs:3431-3438`) — the second of which is what the test's name calls "disconnect".
      **[inspection]** Removing either line passes. *Fix:* rename the test to what it covers, and
      add a `route.rs` case: a routed message for a client that then disconnects must leave no
      entry in `locked_escapes`, since the id is reused by the next connection.
      *Done:* renamed `..._and_passthrough_changes`;
      `a_client_disconnect_cancels_its_pending_escape` drives `route_thread_main` over a real
      local socket the client closes and asserts the escape is gone before `RemoveClient`.
      Killed by deleting the removal in `route.rs`. The `KeyPassthroughChanged` server arm
      remains undriven, as noted.

- [x] **Every `nested_guest` test uses one pane.** All seven build a tracker and drive
      `PaneId::Terminal(1)`, so nothing checks that the tracker keys anything by pane at all.
      **[mutation]** Changing `on_tick`'s suspend arm to `self.last_heard_from.clear()` instead of
      `.remove(&pane_id)` (`nested_guest.rs:68`) — one silent guest forgetting every other guest on
      the machine — leaves all seven green. *Fix:* one test with two panes that suspends the first
      and asserts the second is still `is_tracked` and still gets `SendPing`; and one that
      `remove`s the first and asserts the same.
      *Done:* `suspending_one_guest_leaves_the_other_tracked` and
      `removing_one_guest_leaves_the_other_tracked`. Each killed by the corresponding
      `.clear()`.

- [x] **`ping_interval_ms` and `pong_timeout_ms` are never called by a test.** Both read an env var
      (`ZELLIJ_NESTED_{PING_INTERVAL,PONG_TIMEOUT}_MS`) with a constant fallback
      (`nested_guest.rs:10-22`); the tests compare against the `PING_INTERVAL_MS` /
      `PONG_TIMEOUT_MS` constants instead, so the override branch is dead to the suite and a
      typo'd variable name would ship. `ping_interval_ms` in particular has no test caller at all —
      `on_tick` never consults it, since the ping *cadence* is the background job's
      (`background_jobs.rs:903-909`), which makes the `start + millis(PING_INTERVAL_MS)` in
      `announced_guest_gets_pinged_and_pong_keeps_it_alive` decorative: any instant inside the
      timeout gives `SendPing`. **[inspection]** *Fix:* two tests that set the env var and assert
      the parsed value, and a comment on the decorative argument.
      *Done:* the parse is a pure `millis_from_env(value, default)` tested with `750`, junk
      and `None` without touching the process environment; the two readers are asserted to
      return their defaults with the variables unset. The decorative `PING_INTERVAL_MS` has
      its comment.

### `instruction_context_tests` (`pty`, `pty_write`, `screen`, `server`)

The macro-level finding from pass 1 — `the_special_case_lists_are_current` never calls
`From<&Instruction> for Context`, so it cannot see where a hand-written arm points — applies
unchanged to all four modules here and is not restated. Two things are specific to this batch.

- [x] **`screen::the_special_case_lists_are_current` is the largest instance of that gap, by an
      order of magnitude.** `ScreenInstruction::Resize` maps ten `(Resize, Direction)` pairs to ten
      contexts by hand (`screen/instruction.rs:905-928`), and `only_special` exempts exactly those
      ten plus `HandlePtyBytes` from `every_context_is_reachable` — so eleven of `ScreenContext`'s
      variants are reachable only through arms no test evaluates. **[mutation]** Swapping
      `ResizeIncreaseLeft` and `ResizeIncreaseRight` (`:911`, `:914`), the exact copy-paste error
      this hand-written block invites, leaves all three `screen` tests green. *Fix:* pass 1's
      proposed `special: [(name, expected_context)]` extension is not enough here, because the
      payload chooses the context; `screen` needs a table test over the ten `ResizeStrategy`
      combinations asserting each lands on its own variant.
      *Done:* the `screen` `targets:` table now pins all ten `(Resize, Direction)` pairs, one
      row each. Killed by swapping `ResizeIncreaseLeft`/`ResizeIncreaseRight` in the `From` impl.

- [x] **`pty::the_special_case_lists_are_current` and `pty_write::the_special_case_lists_are_current`
      assert nothing.** Both instantiations declare `special: []; only_special: []`
      (`unit/instruction_context_tests.rs:139-153`), and the test body is two `for` loops over
      those slices, so each is an empty function that always passes. That is correct — both `From`
      impls are pure `from_str` with no hand-written arms (`pty.rs:1003-1013`,
      `pty_writer.rs:27-37`) — but a reader scanning the checklist sees two green boxes that cost a
      test process each and can never go red. **[inspection]** *Fix:* have the macro skip generating
      the test when both lists are empty, or add `assert!(SPECIAL.is_empty() == <the From impl has
      no arms>)`-style intent in a comment so the emptiness is visibly deliberate.
      *Done:* the macro has a first arm for `special: []; only_special: []; targets: []` that
      emits only the two by-name guards (shared through `by_name_guards!`), with a comment saying
      why. `pty` and `pty_write` lose `special_cases_land_on_their_context` and
      `the_special_case_lists_are_current`; `ZELLIJ-SERVER-TESTS.md` and the inventory follow.

### `locked_escape::tests`

- [x] **`locked_escape_matches_only_the_standard_unlock_binding` does not pin *which* mode the
      binding switches to.** It tries `vec![]`, `[NoOp]` and `[unlock, NoOp]` against
      `starts_escape` (`locked_escape.rs:42-53`), but never a `SwitchToMode` to some *other* mode.
      **[mutation]** Relaxing the pattern to `[Action::SwitchToMode { .. }]` leaves it green — so a
      user who has bound Ctrl+g in locked mode to "switch to Pane mode" would have the key quietly
      swallowed into an unlock prefix. *Fix:* add
      `[Action::SwitchToMode { input_mode: InputMode::Pane }]` and one `[Action::SwitchToMode {
      input_mode: InputMode::Locked }]` to the rejecting cases.
      *Done:* `SwitchToMode { Pane }` and `SwitchToMode { Locked }` are rejecting cases. Killed
      by relaxing the pattern to `[Action::SwitchToMode { .. }]`.

- [x] **`locked_escape_resolution_is_per_client_and_stale_tokens_are_harmless` and
      `locked_escape_fallback_updates_mode_and_queues_unlock_before_returning_input` cannot see the
      plugin half of an unlock.** Both wire a real `to_screen` sender and assert on
      `CloseContextMenu` and `ChangeMode`, but leave `to_plugin` as `None` under
      `should_silently_fail: true`, so `resolve_locked_escape`'s two `send_to_plugin` calls
      (`locked_escape.rs:68-85`) — the `UserInput` that records the unlock as an action and the
      `Event::InputReceived` that wakes every subscribed plugin — are dropped unobserved.
      **[mutation]** Deleting both sends leaves all nine tests green. *Fix:* wire a `to_plugin`
      channel the same way `to_screen` is wired and assert both instructions arrive, in order,
      before the `ChangeMode`.
      *Done:* both tests wire `to_plugin` beside `to_screen`;
      `assert_unlock_reported_to_plugins` checks `UserInput { SwitchToMode { Normal } }` then
      `Update([(None, Some(client), InputReceived)])` and nothing else, and the stale-token and
      already-unlocked paths assert the plugin channel stays empty. Killed by deleting both
      `send_to_plugin` calls.

- [x] **`locked_escape_fallback_updates_mode_and_queues_unlock_before_returning_input` never
      asserts what it returns.** The body is `assert!(prepare_input(...).unwrap().is_some())`, so a
      regression that resolved the escape correctly and then handed the screen the *wrong*
      instruction — a different key, a stripped `raw_bytes`, a synthesised action — passes.
      **[inspection]** *Fix:* match the returned value against the instruction that went in, for
      both the `Key` and the `Action` case.
      *Done:* the returned value is `assert_eq!`'d against a clone of the instruction that went
      in, for the `Key`, the interactive `Action` and the cli `Action` case.

- [x] **A cli-client `Action` is never tried, and the guard that excludes one is untested.**
      `prepare_input`'s entry filter (`locked_escape.rs:99-107`) deliberately lets
      `ClientToServerMsg::Action { is_cli_client: true, .. }` through *without* resolving a pending
      escape, so a `zellij action` from a script does not cancel a user's half-typed prefix.
      **[mutation]** Collapsing the arm to `ClientToServerMsg::Action { .. }` leaves all nine tests
      green. *Fix:* add a third case to the fallback test's loop with `is_cli_client: true`,
      asserting the escape is still pending and the mode is still `Locked` afterwards.
      *Done:* the fallback test's loop is `(instruction, should_resolve)` with a third case,
      `is_cli_client: true`, asserting the message is returned unchanged, the pending token is
      still there, the mode is still `Locked`, and neither channel received anything. Killed by
      collapsing the guard to `ClientToServerMsg::Action { .. }`.

- [x] **`locked_escape_is_cancelled_by_mode_configuration_and_disconnect_changes` is named for a
      path it does not take.** Covered by the cross-cutting item above; noted here because the name
      is what a reader trusts. **[inspection]**
      *Done:* with the cross-cutting item — renamed to `..._and_passthrough_changes`, and
      `a_client_disconnect_cancels_its_pending_escape` covers the path the old name promised.

### `logging_pipe::logging_pipe_test`

The group item above covers all seven; these are the additional gaps.

- [x] **The six hand-written tests assert a byte *count*, never which bytes survive.**
      `write_with_endl_in_the_middle_consumes_buffer_up_to_endl_after_flush` (`:125-147`) asserts
      `pipe.buffer.len() == test_buffer2.len()` after four complete lines plus a partial one — the
      same shape as pass 1's `bounded_command_output_is_truncated_and_drained` finding.
      **[mutation]** Replacing `drop(self.buffer.drain(..consumed_bytes))` (`:82`) with a drain of
      the *last* `consumed_bytes` — right count, wrong end, so the partial tail is thrown away and
      the head of an already-logged line is kept and re-logged with the next write — leaves all
      seven green, including the property test, over 8 consecutive runs. *Fix:* assert the residual
      bytes equal `test_buffer2`, not just its length.
      *Done:* `write_with_endl_in_the_middle_...` asserts the residual bytes equal
      `test_buffer2`. Killed by draining the last `consumed_bytes` instead of the first (which
      also kills the new wrap test).

- [x] **`buffer_never_grows_past_the_cap_for_any_write_sequence` never checks that an over-cap write
      is reported.** It calls `let _ = pipe.write(write)` (`:249`) and only inspects the buffer, so
      the `Err(InvalidData)` at `logging_pipe.rs:39-50` — the only signal a plugin's `write_all`
      gets that its log line was dropped — is unasserted. **[mutation]** Turning that branch into
      `self.buffer.clear(); return Ok(buf.len())`, i.e. silent data loss, leaves the module green.
      Separately, **[mutation]** removing the `self.buffer.clear()` on that path (`:45`), which
      leaves a stale partial line to be prepended to the next one, also leaves it green. *Fix:*
      capture the `write` result in the property test and assert `is_err()` exactly when
      `buffer.len() + write.len() > ZELLIJ_MAX_PIPE_BUFFER_SIZE`, and that the buffer is empty
      after an error.
      *Done:* the property test computes `over_cap` before each write, asserts
      `write(..).is_err() == over_cap`, and asserts the buffer is empty after an error. Killed
      by either mutation: the silent `Ok(buf.len())`, and the missing `clear()`.

- [x] **`flush`'s `make_contiguous` is unprotected.** `flush` reads `self.buffer.as_slices().0`
      (`:64`), which is only the whole buffer because of the `make_contiguous()` on the line above;
      after a drain the `VecDeque` can wrap, and then the front slice is a prefix. **[mutation]**
      Deleting `self.buffer.make_contiguous()` (`:62`) leaves all seven green — the six
      hand-written tests each do a single write into a fresh pipe, which never wraps, and the
      property test did not produce a wrapping layout either. *Fix:* a test that writes, flushes,
      then writes again enough to wrap, and asserts the second line is still logged whole.
      *Done:* `a_line_that_wraps_the_ring_buffer_is_still_logged_whole` builds a 16-byte deque,
      logs one line leaving one byte so the head stays put (an emptied deque resets its head),
      appends a second line straight onto the buffer, asserts `as_slices().1` is non-empty as
      the precondition, and then asserts the flush logs it whole. Killed by deleting
      `make_contiguous`.

- [x] **`write_with_incorrect_byte_boundary_does_not_crash` stops at the interesting point.** It
      writes three bytes of a four-byte codepoint and asserts they are retained (`:172-186`), but
      never writes the fourth, so the reason for retaining them — that the completed character is
      then logged — is untested. Worth covering at the same time: a write whose *valid prefix*
      contains a complete line but which ends mid-codepoint (`b"line\n\xf0\x9f\x98"`) has the whole
      line withheld, because `from_utf8` is applied to the entire buffer rather than to its longest
      valid prefix. **[inspection]** The test also ends with a leftover `println!` (`:185`).
      *Fix:* complete the codepoint and assert the line is logged; add the mixed case and pin
      whichever behaviour is intended.
      *Done:* the test goes on to write the fourth byte (still nothing logged) and a newline,
      asserting `["😱"]`. `a_line_ahead_of_a_split_code_point_waits_for_it` pins the mixed case
      as it behaves today: `line\n` is withheld with the split code point and both are logged
      once it completes. The `println!` was already gone.

- [x] **`write_with_many_endl_consumes_whole_buffer_after_flush` and
      `write_with_single_endl_at_the_end_consumes_whole_buffer_after_flush` make the same
      assertion.** Five copies of `"Testing write \n"` versus one; both check `buffer.len() == 0`,
      and with nothing asserted about the log output the extra lines change nothing.
      `write_with_many_endls_consumes_everything_after_flush` (`:190-203`) is the only one that
      exercises empty lines — `flush` logs an empty *middle* line but drops an empty *trailing*
      one (`:72-80`), an asymmetry no test records. **[inspection]** *Fix:* once the log sink is
      observable, keep one of the two and give the third an assertion about how many lines were
      logged and which were blank.
      *Done:* by the log sink from the group item: the two now assert five versus one logged
      line, and `..._many_endls_...` asserts `["Testing write ", "Testing write ", "", "", ""]`
      — the trailing `\n` ends the last line rather than opening a blank one, which is what
      the "asymmetry" is. No further change.

### `nested_guest::tests`

- [x] **`guest_that_never_pongs_suspends_after_timeout` tests one side of the timeout.** It ticks at
      `PONG_TIMEOUT_MS + 1` and at `+ 2`; nothing ticks at exactly `PONG_TIMEOUT_MS`, which is what
      `on_tick`'s `>` (`nested_guest.rs:64-66`) exists to distinguish from `>=`. **[mutation]**
      Changing it to `>=` leaves all seven green. *Fix:* add a tick at exactly the timeout asserting
      `SendPing`, alongside the existing one at `+ 1`.
      *Done:* a tick at exactly `PONG_TIMEOUT_MS` asserts `SendPing` before the `+ 1` tick.
      Killed by `>=`.

- [x] **`removed_guest_stops_being_tracked` cannot tell a removed guest from a suspended one.** It
      announces, removes, and asserts `on_tick` returns `Unknown` — which a *dormant* pane also
      returns. **[mutation]** Deleting `self.dormant.remove(&pane_id)` from `remove`
      (`nested_guest.rs:76-79`) leaves it green, yet a dormant entry that survives removal is a
      permanent leak: the pane's id, once reused, re-announces as `Revived` rather than `New`, and
      `handle_nested_guest_announce` skips the fresh-guest setup — clearing stale choice
      indicators and raising the modal — on that basis (`screen/nested.rs:147-165`). *Fix:* remove
      a *dormant* guest and assert `!is_known`, and assert `on_announce` afterwards returns `New`.
      *Done:* `removing_a_dormant_guest_forgets_it` suspends, removes, asserts `!is_dormant`
      and `!is_known`, and asserts the next `on_announce` is `New`. Killed by deleting
      `self.dormant.remove(&pane_id)`.

### `notifications::notifications_tests`

- [x] **`a_kitty_host_is_detected_from_its_term` cannot tell an exact match from a substring one.**
      It passes `TERM=xterm-kitty`, and the sibling test's `xterm-256color` only rules out matches
      that are wrong in the other direction. **[mutation]** Loosening
      `term == "xterm-kitty"` (`notifications.rs:23`) to `term.contains("kitty")` leaves every
      `notifications` test green. Which one is intended is a real question — kitty's own `TERM` is
      `xterm-kitty`, but `TERM=xterm-kitty-direct` and a `kitty`-suffixed terminfo copy both exist
      in the wild, and today they fall back to OSC 9. *Fix:* decide, then pin it with the case that
      distinguishes the two — `TERM=xterm-kitty-direct` asserting whichever protocol was chosen.
      *Done:* decided for the exact match — `xterm-kitty` is what kitty sets, a real kitty
      behind another `TERM` still has `KITTY_WINDOW_ID`, and OSC 9 is the safe fallback for a
      terminal only *named* after kitty. Pinned with `xterm-kitty-direct`, `kitty` and
      `xterm-kitty2` all resolving to `Osc9`, with the reasoning in a comment. Killed by
      `term.contains("kitty")`.

### `os_input_output::os_input_output_tests`

- [x] **`a_dead_remote_transport_does_not_prevent_a_local_rerun` never reaches the failure it is
      named for.** It drops the broker peer and reruns locally, and the guarantee it exists to
      protect is the `.non_fatal()` on `self.clear_terminal_id(terminal_id)`
      (`os_input_output.rs:1589`), which stops a doomed transport teardown from aborting the
      rerun. **[mutation]** Changing that to `?` leaves the test green: tearing down a remote
      terminal whose socket peer is gone returns `Ok`, so the error branch the test is guarding
      has never been entered. As written the test is a duplicate of the happy path with an extra
      `drop`. *Fix:* register a remote terminal whose teardown genuinely fails — a `RemoteConn`
      over a socket that has been shut down for writes, or a stub backend whose
      `clear_terminal_id` returns `Err` — and assert the rerun still yields a live child.
      *Done:* the test calls `mark_transport_lost()` on the connection before the rerun — what
      the reader does on a hangup, and what makes `send_control` refuse the `Terminate` — and
      asserts `terminate()` fails as a precondition, then that the rerun still yields a local
      child. Killed by `?` in place of the `.non_fatal()`.

- [x] **`a_local_rerun_terminates_the_previous_remote_transport` never asserts a local child was
      started.** It correctly asserts the old broker received `ServerToBrokerMsg::Terminate`, then
      cleans up under `if let Some(child_pid) = child_pid` (`unit/os_input_output_tests.rs:593-595`)
      — so a regression that terminated the remote transport and then failed to spawn locally,
      returning `(reader, None)`, passes and leaves the pane dead. **[inspection]** *Fix:*
      `let child_pid = child_pid.expect("the rerun should have started a local child")` before the
      kill.
      *Done:* `child_pid.expect("the rerun should have started a local child")`.

- [x] **`client_writer_flush::a_queued_exit_reaches_the_socket_before_the_wait_returns` does not
      check the ordering in its name.** After `wait_until_drained` returns true it reads the client
      end with `recv_server_msg`, which has no deadline — so if the wait returned *before* the
      writer thread flushed, the read simply blocks until it does, and the test still passes.
      **[mutation]** Moving `live_writers.deregister()` from the end of the writer thread
      (`os_input_output.rs:451`) to its start, so the wait is satisfied before a single byte is
      written, leaves it green — and the whole point is that the server may exit the moment the
      wait returns, at which point an unflushed client sees a hangup instead of its exit reason.
      The two sibling tests that would catch an outright broken `wait_until_drained` use no socket
      at all. *Fix:* set a zero or very short recv timeout on the receiver before reading, so the
      message must already be buffered when the wait returns.
      *Done:* the read now has a 1ms receive timeout, so an unflushed writer reads as `Idle`
      rather than being waited for. The ordering itself is pinned by
      `the_wait_does_not_return_while_a_writer_is_still_blocked_on_the_socket`: the socket is
      filled from the server side until `WouldBlock`, the writer blocks on the `Exit`, a 200ms
      wait must report *not* drained, the client then reads the filler, the wait reports
      drained, and the `Exit` is on the socket. Passes 10/10 in a loop. Killed by moving
      `deregister()` to the top of the writer thread.

### Production observations

Kept separate from the test findings; none is a regression, and none is fixed by this audit.

- `NestedGuestTracker::is_dormant` and `is_known` (`nested_guest.rs:83-88`) have no caller outside
  the tests — the whole workspace only reaches `is_tracked`, `on_announce`, `on_pong`, `on_tick`
  and `remove`. Two of the seven tests assert on accessors production never consults; the
  observable consequence of suspension is `on_tick` returning `Unknown` (which stops the ping job)
  and the next `on_announce` returning `Revived`.
- `LoggingPipe::flush` applies `from_utf8` to the entire buffer (`logging_pipe.rs:64`), so a write
  that ends mid-codepoint withholds every complete line already in the buffer until the character
  is finished. Bounded by the 16 KiB cap rather than unbounded, but a plugin that never completes
  the character loses those lines to the cap's `buffer.clear()`.

## Method (pass 2)

Every test in the pass was read against the production code it covers. **Fifteen mutations were
applied across four batches, and all fifteen survived**, each confirming the finding it was aimed
at. Batching disjoint mutations kept the cost to four compiles; each narrowed run was ~0.34s of
test time inside a ~10s build. All mutations were reverted and the tree verified clean — no `.rs`
file is modified by this audit.

## Pass 3 — items 81-130 (`ZELLIJ-SERVER-TESTS.md` lines 104-154)

Audited 2026-09-08. 50 tests: `os_input_output_tests::client_writer_flush` (2),
`os_input_output_tests::coalescing_send_queue` (11), `os_input_output_tests` top level (13),
`os_input_output_unix::{tests,callback_tests}` (6), `pane_groups::tests` (18).
All 50 pass at HEAD (67 tests in 0.31s narrowed,
`cargo x test -p zellij-server -E 'test(/^os_input_output/) or test(/^pane_groups::/)'`).

**21 sound, 29 need work, plus 3 gaps that belong to no single test.**

**All 19 items below are done (2026-09-08)**; each carries a *Done* note saying what landed and
which mutation now kills it. Three tests were renamed and ten deleted (see the `pane_groups`
item), and `ZELLIJ-SERVER-TESTS.md` follows:
`closing_an_empty_queue_ends_a_pop_that_was_already_waiting`,
`a_close_racing_a_pop_still_drains_everything_queued` and
`stop_firecracker_session_is_independent_of_a_failed_pane_teardown`. Two tests are new:
`pane_groups::tests::launching_the_plugin_sends_fill_plugin_cwd_to_pty` and
`os_input_output_unix::tests::try_write_to_fd_propagates_an_error_that_is_not_eagain`. Every
mutation named in a *Done* note was re-applied after the fix and the named test went red; all
were reverted. Two things reach outside the tests: `SEND_QUEUE_CONTROL_CAPACITY` is 8 under
`cfg(test)`, and `UnixPtyBackend` gains a `#[cfg(test)]` `configured_termios()` accessor. The
third production observation below (the shutdown order) is now checked by the integration suite,
though the note stands as a description of the code.

**The batch closes three groups cleanly and cuts a fourth.** `client_writer_flush` is complete
here (pass 2 took four of six), which retires pass 2's provisional note: the lower bound on
`wait_until_drained`'s deadline does not exist anywhere in the group, so pass 2's reading of
`draining_is_reported_once_every_writer_is_done` was right for the wrong reason — see the
cross-cutting item. `coalescing_send_queue` and `os_input_output_unix` are whole. But
`pane_groups::tests` is cut: 18 of its 20 are here and
`toggle_pane_id_{adds_new_pane,removes_existing_pane}` fall to pass 4, which matters because
`toggle_pane_id_in_group` is one of the four callers of the never-observed `launch_plugin` below.
The top-level firecracker family is complete (pass 2 took two of eight).

Sound, no action: `coalescing_send_queue::{blocking_pop_wakes_on_push,
control_messages_stay_fifo_and_precede_the_render, control_queue_fills_while_renders_still_coalesce,
drain_order_is_control_then_pane_renders_then_full_render, initial_and_scrollback_pane_renders_stay_fifo,
pane_renders_coalesce_per_pane, push_after_close_is_dropped, renders_coalesce_to_the_newest}`,
`firecracker_panes_are_refused_a_warp_and_remote_panes_are_not`,
`rename_firecracker_session_updates_the_session_key`, `spawn_and_read_output`,
`os_input_output_unix::callback_tests::terminal_exit_callback_preserves_id_status_and_command`,
`os_input_output_unix::tests::{original_termios_reaches_both_local_pane_paths,
releasing_a_remote_pane_does_not_deadlock_on_the_transport_table}`,
`pane_groups::tests::{get_client_pane_group_returns_correct_panes,
get_client_pane_group_returns_empty_set_for_nonexistent_client,
should_launch_plugin_returns_true_when_first_pane_added,
should_launch_plugin_returns_true_when_empty_to_non_empty,
should_launch_plugin_returns_false_when_empty_to_empty,
should_launch_plugin_returns_false_when_non_empty_to_empty,
should_launch_plugin_returns_false_when_non_empty_to_non_empty}`.

Three deserve a note for being unusually good. `releasing_a_remote_pane_does_not_deadlock_on_the_transport_table`
states the regression it exists for — an `if let` scrutinee holding a `Mutex` guard across a
re-lock — reproduces it on the thread that deadlocked, and asserts the *effect* (the pane leaves
the table) rather than only that the call returned. `original_termios_reaches_both_local_pane_paths`
re-execs the test binary as a real supervisor over the production fd-3 handoff, runs both the
detachable and in-process paths, and asserts `terminal_id_to_host.contains_key(&id) == detachable`
so the detachable path cannot silently fall back — the failure mode that would otherwise make the
whole test vacuous. And `initial_and_scrollback_pane_renders_stay_fifo` is the only test that pins
`slot_for`'s two negative conditions; dropping `scrollback: None` from the pattern, so a
scrollback-carrying update coalesces away, is killed by it alone.

`pane_groups::tests` is the outlier in the other direction: **11 of its 18 need work**, ten of
them for one reason — see the group item below.

### Cross-cutting gaps

- [x] **`pane_groups`' only side effect is never observed.** All 18 tests build `ThreadSenders`
      with `should_silently_fail: true` (`pane_groups.rs:203-208`), so `launch_plugin`
      (`:149-195`) — the `PtyInstruction::FillPluginCwd` that opens the multiple-select plugin,
      and the entire reason `should_launch_plugin` (`:130-148`) exists — is dropped unobserved.
      **[mutation]** Replacing `launch_plugin`'s body with an early return leaves all 18 green:
      a `PaneGroups` that computes perfectly when to launch the plugin and then never launches it
      passes its own suite. The five `should_launch_plugin_*` tests do pin the predicate, but they
      call the private helper directly, so nothing connects the predicate to the effect. *Fix:* a
      test with a real `ThreadSenders` and a pty receiver asserting one `FillPluginCwd` arrives
      with the floating-pane geometry from `:164-176`. Two cases the helper's own tests cannot
      reach, because all five are single-client: the loop at `:136-146` scans *every* client in
      `previous_groups`, so client 2 going empty→non-empty makes client 1's `add_pane_id_to_group`
      launch the plugin; and `group_and_ungroup_panes_for_all_clients` (`:97-126`) runs that scan
      once per client and then launches for `all_connected_clients.first()`, which is a `HashMap`
      key order.
      *Done:* `launching_the_plugin_sends_fill_plugin_cwd_to_pty` builds a real `to_pty` sender and asserts one `FillPluginCwd` with the `:164-176` geometry for a client's first pane, for a second client going empty→non-empty, and once for a group-for-all — and nothing on a non-empty group growing. Killed by the early return.

- [x] **`LiveWriters::wait_until_drained`'s deadline argument is untested by the whole group.**
      All seven `client_writer_flush` tests pass a deadline; none asserts the wait respected it.
      **[mutation]** Clamping `remaining` to 1ms inside the loop (`os_input_output.rs:353-356`),
      so the wait gives up after 1ms whatever it was asked for, leaves all seven green — 5/5 runs.
      The deadline is the whole contract: `lib.rs:2363` calls it as
      `flush_client_writers(CLIENT_WRITER_FLUSH_TIMEOUT)` with 500ms (`lib.rs:62`), and that
      budget is what gives a slow-but-alive client time to receive its `Exit` before the process
      exits. `waiting_gives_up_rather_than_holding_the_process_open` (`:1327-1342`) is where the
      bound belongs and is the test this item accounts for: it asserts only
      `started.elapsed() < Duration::from_secs(2)` against a 50ms deadline — 40x slack, no lower
      bound — so it cannot tell "gave up at its deadline" from "ignored the deadline". *Fix:*
      assert `started.elapsed() >= Duration::from_millis(50)` as well, exactly as the sibling
      `a_silent_peer_reads_as_idle_once_the_deadline_passes` (`:1433-1462`) already does for the
      receive timeout, comment and all.
      *Done:* `waiting_gives_up_rather_than_holding_the_process_open` asserts `waited >= 50ms` as well. Killed by clamping `remaining` to 1ms.

- [x] **The three signal methods cannot fail, so their tests assert nothing.**
      `kill`, `force_kill` and `send_sigint` (`os_input_output_unix.rs:800-813`) are each
      `let _ = kill(process_pid(pid)?, Some(Signal::X)); Ok(())` — the errno is discarded, so the
      `Result` is `Ok` by construction once the pid resolves, and the
      `.expect("kill should succeed")` in `kill_sends_sighup_to_process` (`:374-393`),
      `force_kill_sends_sigkill_to_process` (`:395-411`) and `send_sigint_to_process` (`:413-429`)
      can never fire. Each test then sleeps 100ms and runs `child.kill()` + `child.wait()`
      unconditionally, so the child dies regardless of what the method sent. **[mutation]**
      Pointing all three at `Signal::SIGCONT` leaves all three green — as would sending nothing.
      *Fix:* drop the belt-and-braces `child.kill()`, `child.wait()` the process, and assert
      `std::os::unix::process::ExitStatusExt::signal()` is the signal under test (`sleep 60` dies
      on SIGHUP and SIGKILL; give `send_sigint_to_process` a child that does not ignore SIGINT).
      Keep a bounded wait so a child that survives fails the test instead of hanging it.
      *Done:* all three spawn `sleep 60`, wait bounded (5s, then kill and panic) and assert `ExitStatusExt::signal()` is `SIGHUP`/`SIGKILL`/`SIGINT`; the unconditional `child.kill()` is gone. Each is killed by pointing its method at `SIGCONT`.

### `os_input_output_tests::client_writer_flush`

- [x] **`shutting_a_client_down_unblocks_the_router_reading_it` fails as a harness timeout, not as
      an assertion.** `router.join()` (`:1415`) is unbounded, so if `shutdown_reads`
      (`os_input_output.rs:391-397`) stops producing the EOF the test hangs rather than failing.
      **[mutation]** Replacing the `libc::shutdown` call with a no-op turns the test into a 60s
      nextest kill — and this is exactly the shape AGENTS.md tells the next reader to dismiss as
      contention when the suite runs at `-j 16`, so the regression this test exists to catch (one
      stranded router thread and open socket per probe, until the listen backlog is permanently
      full) would read as a flake. *Fix:* run the read on a channel and `recv_timeout` it, so a
      router that never woke fails by name, the way
      `releasing_a_remote_pane_does_not_deadlock_on_the_transport_table`
      (`os_input_output_unix.rs:1538-1550`) already does for its own hang.
      *Done:* the router sends its read outcome on a channel and the test `recv_timeout`s it at 5s. The `shutdown` no-op now fails by name in 5s instead of a 60s kill.

### `os_input_output_tests::coalescing_send_queue`

- [x] **`pop_returns_none_when_closed_while_blocked` does not check the "while blocked" in its
      name.** It spawns a popper, sleeps 50ms, closes, and asserts `None` (`:900-911`) — which a
      `pop` that never blocked at all would also produce. **[mutation]** Making `pop` return `None`
      whenever the queue is empty, open or not (`os_input_output.rs:293-296`), leaves it green;
      the sibling `blocking_pop_wakes_on_push` is what kills that mutant. So the test is not
      wrong, it is redundant with its sibling under a name that promises more, and the 50ms sleep
      is not load-bearing. *Fix:* have the popper report that it is about to block over a channel
      and assert it had not returned before `close()` — or rename to say it covers "closing an
      empty queue ends the pop" and drop the sleep.
      *Done:* renamed `closing_an_empty_queue_ends_a_pop_that_was_already_waiting`; the popper hands over a marker right before `pop`, the sleep is gone, and the doc says the blocking itself is `blocking_pop_wakes_on_push`'s.

- [x] **`close_drains_before_ending` asserts a strict subset of two siblings.** It pushes one
      control and one render, closes, and pops both then `None` (`:876-885`).
      `control_messages_stay_fifo_and_precede_the_render` (`:843-854`) already covers
      queued-before-close draining in order, and `push_after_close_is_dropped` (`:913-923`) covers
      the other half of `close`'s contract. No mutation kills this test without killing one of
      those first. **[inspection]** *Fix:* delete it, or give it the case the pair does not cover
      — a `pop` racing a concurrent `close` on a non-empty queue, which is the actual shutdown
      shape and the only reason the drain-before-`closed` ordering in `pop` (`:283-295`) matters.
      *Done:* replaced by `a_close_racing_a_pop_still_drains_everything_queued`: a capacity's worth of numbered control messages, a popper draining to `None` while the main thread closes, and the exact sequence asserted.

- [x] **`queue_agrees_with_a_sequential_model` leaves two of `slot_for`'s inputs outside the
      model.** `push_control` asserts `SendQueuePush::Queued` unconditionally (`:1048`), so the
      `ControlQueueFull` arm (`os_input_output.rs:270-272`) can never appear in a run — the state
      machine would fail on it rather than model it — and no rule ever draws a `PaneRenderUpdate`
      with `scrollback: Some(_)`, which is the second of `slot_for`'s two negative conditions
      (`:226-231`). A property test whose name claims agreement with a sequential model should
      cover the whole message space it can construct. **[inspection]** *Fix:* model the control
      FIFO's capacity (a smaller `#[cfg(test)]` capacity keeps the runs short) and add a
      scrollback-carrying rule that pushes into the FIFO half of the model.
      *Done:* `SEND_QUEUE_CONTROL_CAPACITY` is 8 under `cfg(test)`; a shared `push_fifo` models `ControlQueueFull` for every FIFO-path message, and `push_scrollback_pane_render` draws `scrollback: Some(_)`. Killed by dropping `scrollback: None` from `slot_for` and by an off-by-one on the capacity check.

### `os_input_output_tests` (top level)

- [x] **`failed_firecracker_pane_termination_does_not_skip_provider_shutdown` re-implements the
      sequence it is named for.** It calls `clear_terminal_id` and then `stop_firecracker_session`
      as two statements (`:334-336`), so what it proves is that the second call works after the
      first returned `Err` — not that production makes the second call, nor that it makes it
      *after* the panes are closed. The real sequence is `lib.rs:2347-2357`:
      `begin_firecracker_shutdown` → drop the session metadata (which joins the pty thread and
      closes every pane through `clear_terminal_id`) → `shutdown_pty_hosts` → 
      `stop_firecracker_session`. **[mutation]** Moving `stop_firecracker_session` to *before*
      `begin_firecracker_shutdown`, so the session's VMs are stopped while its panes are still
      live, leaves this test green — and leaves all six firecracker integration tests green too
      (`zellij-integration-tests/tests/firecracker_{shutdown,close}.rs`), because they count the
      stops and never check when they happened. Deleting the call outright *is* caught, by
      `firecracker_shutdown::a_session_exit_stops_its_firecracker_vms_once`; the ordering is not
      caught anywhere. *Fix:* have this test assert the ordering it is named for — a pane whose
      teardown fails, driven through the shutdown path rather than beside it — or, if the unit is
      the right level, rename it to what it checks (`stop_firecracker_session` is independent of
      a failed `clear_terminal_id`) and add the ordering assertion to the integration test, whose
      fake provider already records the stop.
      *Done:* renamed `stop_firecracker_session_is_independent_of_a_failed_pane_teardown` with a doc saying so. The ordering lives with the integration suite: `FakeServerOsApi` counts `firecracker_stops_with_live_panes`, and `a_session_exit_stops_its_firecracker_vms_once` asserts it is 0. Killed by moving `stop_firecracker_session` ahead of `begin_firecracker_shutdown` in `lib.rs`.

- [x] **`stopping_the_session_cancels_an_ensure_in_flight` cannot see which of three checks fired,
      and fails as a harness timeout.** `resolve_target` refuses a stopping session at
      `os_input_output.rs:644` (before the ensure), at `:667` (after it), and mid-flight through
      the `&self.firecracker_stopping` it hands `Provider::ensure` — all three with the same
      "shutting down" message, which is all the test matches (`:254`). If the 200ms sleep at
      `:242` does not get the ensure started, the pre-check answers and the test still passes.
      **[mutation]** Making `ensure_with_progress` ignore the flag it is given proves the
      in-flight path *is* being taken today — but the test then fails by hanging on
      `resolver.join()` (`:245`) for the provider's full `sleep 30`, killed by nextest at 30s, so
      its own `start.elapsed() < Duration::from_secs(5)` assertion (`:247-250`) is unreachable.
      *Fix:* join through a channel with a `recv_timeout`, and have the provider script append a
      marker before its `sleep` that the test asserts, so "the ensure was in flight" is checked
      rather than assumed.
      *Done:* the provider touches an `ensuring` marker before its `sleep 30`, the test waits for it (bounded) instead of sleeping 200ms, and reads the resolver's result over a `recv_timeout(5s)` channel. Ignoring the flag now fails by name in 5s.

- [x] **`server_termios_argument_distinguishes_defaults_and_invalid_json` never passes a valid
      termios.** Its four inputs are `None`, `"null"` and three malformed strings (`:1501-1509`),
      so `get_server_os_input_from_termios_arg`'s only interesting line — handing the decoded
      snapshot to `get_server_os_input_with_termios` (`os_input_output.rs:1793`) — is never
      exercised with a snapshot. **[mutation]** Passing `None` there instead of `snapshot`, so a
      server started with `--server-termios <json>` silently ignores the client's terminal
      settings, leaves it green. The sibling that would notice,
      `original_termios_reaches_both_local_pane_paths`, calls `with_termios` directly and never
      goes through the argument. *Fix:* add a case that serializes a `WireTermios` with a
      distinctive flag, passes the JSON, and asserts the resulting backend carries it.
      *Done:* a snapshot with `ICRNL`/`ICANON` set and `ECHO` cleared goes through as JSON and is read back off the backend by a `#[cfg(test)]` `configured_termios()`; `"null"` is asserted to carry none. Killed by passing `None` at `:1793`.

- [x] **`tcgetpgrp_returns_foreground_group`'s real assertion is conditional.**
      `assert_eq!(ProcessId::try_from(fpgid).unwrap(), child_pid)` sits inside
      `if let (Some(fpgid), Some(child_pid))` (`:779-785`), and the poll loop's readiness test
      treats `(Some(p), None)` as ready on `p > 0` (`:766`), so a `spawn_terminal` that returned
      no child pid degrades the whole test to "some positive pgid". Same shape as pass 2's
      `a_local_rerun_terminates_the_previous_remote_transport` finding. **[inspection]** *Fix:*
      `let child_pid = child_pid.expect("spawn_terminal should report the child's pid")` before
      the loop, and make the loop's readiness the equality itself.
      *Done:* `child_pid` is `expect`ed before the loop, the loop's readiness is the equality, and the final `assert_eq!` is unconditional.

- [x] **`get_cwd` asserts only that *a* path came back.** It passes the test process's own pid and
      checks `is_some()` (`:360-370`). **[mutation]** Returning a constant `Some(PathBuf::from("/"))`
      from `get_cwd` (`os_input_output.rs:1440-1456`) leaves it green — yet this is what the new-pane
      cwd inheritance and the session serializer read. *Fix:* assert the result equals
      `std::env::current_dir()`, and add the `None` case (a pid that does not exist) which the
      `if let` chain's fallthrough at `:1455` exists for and which no test reaches.
      *Done:* asserts equality with `current_dir()` (canonicalised) and `None` for `i32::MAX`, above any `pid_max`. Killed by returning a constant path.

- [x] **`firecracker_resolution_keeps_an_unresolved_command_for_reruns` checks the rerun
      round-trip with a bare `is_ok()`.** `assert!(server.resolve_target(rerun, None).is_ok())`
      (`:43`) is the only assertion about re-resolving, so a rerun command that resolves to a
      *different* VM, cwd or endpoint passes — which is the whole point of keeping it unresolved.
      **[inspection]** *Fix:* re-resolve and `assert_eq!` the second `(resolved, rerun, endpoint)`
      against the first.
      *Done:* the rerun is re-resolved and the whole `(resolved, rerun, endpoint)` triple is `assert_eq!`ed against the first.

- [x] **`two_quick_renames_leave_the_provider_keyed_on_the_last_name` carries another test's doc
      comment.** Its first paragraph (`:168-170`) — "A provider that refuses the rename leaves the
      server on the old name too" — describes `a_refused_rename_keeps_the_old_firecracker_session_key`
      (`:257-282`), not this test, which is about rename ordering. The assertions themselves are
      sound: repointing `rename_firecracker_session_now` so it never adopts the new key kills both
      rename tests. **[inspection]** *Fix:* move the stray paragraph to the test it describes.
      *Done:* the paragraph moved onto `a_refused_rename_keeps_the_old_firecracker_session_key`.

### `os_input_output_unix`

- [x] **`termios_supervisor_entry` is not a test.** With `ZELLIJ_TEST_TERMIOS_SUPERVISOR` unset —
      which is every run except the re-exec — its body is empty (`:1292-1297`), so it is a green
      checklist box costing a test process that can never go red. It exists because
      `original_termios_reaches_both_local_pane_paths` re-execs the test binary at it (`:1326-1329`),
      which is a good technique, but a reader scanning `docs/TEST_INVENTORY.md` sees a test.
      **[inspection]** Same shape as pass 2's `pty::the_special_case_lists_are_current` item.
      *Fix:* it cannot be deleted, so give it a doc comment saying it is the supervisor entry point
      for the test below and is a no-op otherwise — and consider `assert!` on the env var being
      absent so the empty path is at least deliberate.
      *Done:* a doc comment names it as the re-exec entry point for `original_termios_reaches_both_local_pane_paths` and the unset-variable path as a deliberate no-op (an `assert!` there would fail the re-exec).

- [x] **Neither `try_write_to_fd` test covers the error arm.** `try_write_to_fd`
      (`os_input_output_unix.rs:445-457`) treats `EAGAIN` and a zero-length write as "stop here,
      re-queue the rest" but propagates every other errno — which is how `PtyWriter` learns a
      pane's fd is gone. **[mutation]** Changing `Err(e) => return Err(e.into())` (`:453`) to
      `Err(_) => break`, so a dead fd reads as a full buffer and the pane's writes are silently
      dropped forever, leaves both `try_write_to_fd_returns_partial_on_full_buffer` and
      `try_write_to_fd_returns_zero_on_stuck_pty` green. *Fix:* a third test that closes the slave
      (or writes to a closed fd) and asserts `is_err()`. The `EINTR` retry (`:451`) is also
      untested but is not worth a fixture.
      *Done:* `try_write_to_fd_propagates_an_error_that_is_not_eagain` writes to a read-only `/dev/null` fd and asserts the `EBADF` error. Killed by `Err(_) => break`.

- [x] **`try_write_to_fd_returns_partial_on_full_buffer` asserts a count, never which bytes.**
      `assert!(written > 0 && written < size)` (`:1456-1459`) is the whole check, so nothing pins
      that the accepted bytes are the *prefix* of the buffer — the contract the caller relies on
      when it re-queues `&data[written..]`. Same shape as pass 1's
      `bounded_command_output_is_truncated_and_drained` finding. **[mutation]** Returning
      `Ok(written.saturating_sub(1))` — the right shape, one byte under, which would duplicate a
      byte into every pane on every partial write — leaves both tests green. *Fix:* the test
      already writes a distinguishable `(i % 256)` payload; drain the slave and assert the bytes
      read back are `data[..written]`.
      *Done:* the slave is drained non-blocking and the bytes read back must end with `data[..written]`. Killed by `Ok(written.saturating_sub(1))`.

### `pane_groups::tests`

- [x] **`pane_groups_stay_duplicate_free_and_track_the_model` is blind to ordering, which is
      production-visible.** `groups_are_duplicate_free_and_match_the_model` (`:679-709`) collects
      both the real group and the model into `HashSet`s before comparing, but `clone_inner`
      returns the `Vec` in insertion order and callers depend on it — `unit/screen_tests/mouse.rs:659`
      asserts an exact `vec![Terminal(4), Terminal(3)]`. **[mutation]** Sorting the group in
      `add_pane_id_to_group` (`pane_groups.rs:71-73`) leaves the state machine green, and the
      twelve hand-written tests with it. *Fix:* compare the `Vec`s directly; the model already
      maintains insertion order through `push_unique`, so this is a one-line change that makes
      the invariant strictly stronger. (The state machine is otherwise the module's real
      coverage: removing the client key in `clear_pane_group` instead of clearing its vec is
      caught by its client-set assertion and by nothing else.)
      *Done:* the invariant compares the group to the model as slices. Killed by sorting in `add_pane_id_to_group`.

- [x] **Ten hand-written tests are subsumed by that state machine.** `new_creates_empty_pane_groups`,
      `clone_inner_returns_copy_of_internal_map`, `clear_pane_group_clears_existing_group`,
      `clear_pane_group_handles_nonexistent_client`, `add_pane_id_to_group_adds_new_pane`,
      `add_pane_id_to_group_does_not_duplicate`, `group_and_ungroup_panes_adds_and_removes_correctly`,
      `override_groups_with_replaces_all_groups`, `multiple_clients_independent_groups` and
      `pane_id_variants_work_correctly` (`:218-475`) each drive one rule the machine already has,
      against an invariant that checks the same thing more strongly and over more histories —
      the machine draws both `PaneId` variants from a shared id pool, so it covers the
      discriminant `pane_id_variants_work_correctly` exists for, and its client-set assertion
      covers what the two `clear_pane_group_*` tests assert. **[inspection]** No mutation kills
      any of the ten without killing the state machine first. The two exceptions, kept above as
      sound, are `get_client_pane_group_returns_correct_panes` and
      `..._returns_empty_set_for_nonexistent_client`: the invariant reads through `clone_inner`,
      so those two are the only coverage of `get_client_pane_group` (`:35-40`), which
      `screen/mod.rs:4417` and four `screen/thread_main.rs` sites call. *Fix:* delete the ten and
      spend the runtime on the plugin-launch gap above; `clone_inner_returns_copy_of_internal_map`
      in particular never checks the independence its name claims — nothing mutates the clone and
      re-reads the original.
      *Done:* the ten are deleted; `pane_groups::tests` is now the two `get_client_pane_group_*`, the two `toggle_pane_id_*`, the five `should_launch_plugin_*`, the launch test and the state machine.

### Production observations

Kept separate from the test findings; none is fixed by this audit.

- `kill`, `force_kill` and `send_sigint` (`os_input_output_unix.rs:800-813`) discard `kill(2)`'s
  result with `let _ =`, so their `Result<()>` is `Ok` for a pid that resolves and the errno is
  lost. Every caller's `?` and `.non_fatal()` on these three is decorative; a pane whose process
  survived a `SIGHUP` because of `EPERM` is indistinguishable from one that died.
- `should_launch_plugin` (`pane_groups.rs:130-148`) scans every client in `previous_groups` and
  returns true if *any* went empty→non-empty, then ORs in whether the acting client is new. The
  per-client argument therefore only affects the second clause, so one client's first pane
  launches the multiple-select plugin for another client's operation.
  `group_and_ungroup_panes_for_all_clients` (`:97-126`) calls it once per client, making the scan
  quadratic, and then launches for `all_connected_clients.first()` — a `HashMap` iteration order,
  so which client gets the plugin is not deterministic.
- The server shutdown (`lib.rs:2347-2357`) is order-dependent — the VMs must stop after the panes
  are closed — and nothing checks the order, in this suite or in the integration suite. Moving
  `stop_firecracker_session` ahead of the pane teardown passes both.
- `LiveWriters::deregister` (`os_input_output.rs:338-344`) uses `saturating_sub`, so an unbalanced
  deregister silently under-counts and `wait_until_drained` reports drained early. Nothing
  currently double-deregisters; the saturation hides it if something starts to.

## Method (pass 3)

Every test in the pass was read against the production code it covers. **Twenty mutations were
applied across six batches; eight survived outright**, each confirming the finding it was aimed
at, and three more survived the tests they were aimed at while being killed by a sibling — which
is itself reported above (`pop_returns_none_when_closed_while_blocked`, the two
`clear_pane_group_*` tests, and `failed_firecracker_pane_termination_does_not_skip_provider_shutdown`,
whose deletion-mutant is caught only by the integration suite). Batching disjoint mutations kept
the cost to six compiles; each narrowed run was ~0.31s of test time inside a ~10s build, and two
mutations were additionally checked against `cargo x integration-test -E 'binary(/firecracker/)'`.
All mutations were reverted and the tree verified clean — no `.rs` file is modified by this audit.
