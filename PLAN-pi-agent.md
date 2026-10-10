# Pi integration

The integration targets the installed Pi 1.0.0 interactive CLI.

- The shared launcher supports model, thinking level, opening prompts, continue,
  and session resume. Permission modes are rejected because Pi delegates them to extensions.
- The planner emits an internal extension placeholder. Host spawn preparation
  writes the shared template atomically into Yinz's native data directory and resolves it.
  Creation failures reject the spawn. Server startup does not provision Pi.
- Remote launches remove only Yinz's generated extension argument. SSH uses hooks
- The hook installer honors `PI_CODING_AGENT_DIR`, with `~/.pi/agent` as its default.
  It supports user-level install, status, uninstall, backups, dry-run, and foreign-file protection.
- The extension shares ownership and an ordered report queue across copies and reloads.
  Shutdown releases ownership and drains reports within five seconds. Each reporting
  subprocess also has a five-second limit. Ordinary callbacks do not wait for reporting.
- Each accepted prompt receives a unique turn ID. `agent_settled` confirms completion.
  The final assistant message distinguishes success, failure, and abort.
- Blocking UI prompts report waiting. Dismissal and manual compaction restore the
  actual activity state. Invisible waits in third-party extensions cannot be observed.
- Pi participates in readiness, prompt acknowledgement, and idle waits. Existing
  conversation, fingerprint, turn, input revision, and bracketed-paste checks remain in force.
  `/new` uses conversation reset. Other slash commands and shell input require `send-keys`.
- `hooks: false` disables injection, not user-installed Pi extensions.
- Restart removes prompts and session selectors, preserves supported launch options,
  and rejects noninteractive modes and ambiguous argument arities.

## Verification

Rust regression tests cover launch, remote arguments, display filtering, restart,
installation, and prompt control. `src/unit/pi_extension.mjs` runs against the
rendered extension through the hook installer tests. It covers duplicate loading,
reload, session replacement, report ordering, failure recovery, timeout, waiting,
compaction, final text, success, failure, and abort.

The integration suite uses a fake Pi PTY to check host extension preparation and
prompt correlation. Run `python src/unit/pi_smoke.py` for an optional isolated
startup/reload/shutdown check against installed Pi. It makes no model request.

Build this worktree's plugins and binary with `cargo x build`. Run focused tests,
then `cargo x ci check` for formatting, lint, unit tests, and integration tests.

See [AGENT_CLI.md](docs/AGENT_CLI.md) for user instructions.
