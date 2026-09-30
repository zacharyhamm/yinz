# Codex binding and hook-routing evidence

These were captured by hand during the run. The registry directory was deleted at teardown.

## `/tmp/yinz-1000/codex/ui-ODnYbU/binding.json` (pane `terminal_4`)

At spawn (11:02), after the TUI's `thread/start` response:

```json
{"thread_id":"01a0ede6-d40f-70d3-b91c-f73db1a83d8b","env":{"YINZ_SESSION_NAME":"considerate-aardvark","YINZ_PANE_ID":"4","YINZ_PANE_REF":"bc4043a19f6e","YINZ_SOCKET_DIR":"/run/user/1000/yinz"}}
```

After the first prompt (11:03:38), the file was rewritten to point at the hidden title-generation thread:

```json
{"thread_id":"01a0ede8-4a80-73d1-a5e2-486d5c2493ee","env":{"YINZ_SESSION_NAME":"considerate-aardvark","YINZ_PANE_ID":"4","YINZ_PANE_REF":"bc4043a19f6e","YINZ_SOCKET_DIR":"/run/user/1000/yinz"}}
```

`agent status terminal_4` kept reporting `session_id` `01a0ede6-…`, the real conversation. Its Stop hook had
no binding and was dropped. The Yinz log line from that period was:

```
hook report=2292eac2-… kind=codex event= session= pane= bytes=0 stage=codex_route duration_us=386 outcome=unbound
```

After a second manual turn, `turn/start` on the real thread rebound the pane to `01a0ede6-…`, and hooks routed again.

## Replaying a hook by hand

- `yinz hocus codex-report` with a synthetic `UserPromptSubmit` or `Stop` for `01a0ede6-…`, sent while the binding pointed at
  `01a0ede8-…`, exited 0 and did not change the pane. The binding lookup missed, and the fallback used the caller's own pane environment.
- `env -u YINZ_PANE_REF yinz action agent-report --pane-id terminal_4 --hook-json --kind codex` with `Stop` set the pane to Idle.
  This shows that the screen side accepts the report, so the loss happens in routing.
- `YINZ_PANE_ID=4 yinz action agent-report …` run from inside pane 0 went to **pane 0**, because `YINZ_PANE_REF` retargets it.
  That is noted as a hazard in the report, not as a Codex bug.

## Codex `/hooks` view (terminal_4)

All Yinz hook events were installed and active: SessionStart (2, Yinz plus the ponytail plugin), UserPromptSubmit (2),
PreToolUse, PermissionRequest, PostToolUse, PreCompact, PostCompact, SessionEnd, SubagentStart (2), SubagentStop, and Stop.
So the hooks were trusted and running. The missing reports are not a trust problem.

## Environment inside Codex tool commands (sandboxed pane, `--permission-mode on-request`)

```
$ env | grep -E "^YINZ|^TMPDIR|^XDG_RUNTIME"
XDG_RUNTIME_DIR=/run/user/1000
$ yinz agent list --json
Error: no session selected; pass --session SESSION
$ yinz --session considerate-aardvark agent list --json
Error: cannot connect to session considerate-aardvark: Operation not permitted (os error 1)
$ yinz --session considerate-aardvark action context
There is no active session!
```

## Codex log rows showing the title thread's `thread/start`

These rows were read from `~/.codex/logs_2.sqlite` during the run at 11:05. Codex pruned them before this evidence was archived,
so they are quoted here, truncated as displayed:

```
2026-09-29 11:03:38|app_server.request{otel.kind="server" otel.name="thread/start" rpc.system="jsonrpc" rpc.method="thread/start" rpc.transport="unix_socket" rpc.request_id=temporary-structured-e4d2a12f-db79-4064-86d7-248aad66e67e app_server.connection_id=33 app_server.api_version="v2" app_server.client_name="codex-tui" app_server.client_version="0.158.0"}:app_server.thread_start.create_thread{…
2026-09-29 11:03:38|session_loop{thread_id=01a0ede8-4a80-73d1-a5e2-486d5c2493ee}: Submission sub=Submission { id: "01a0ede8-4aae-70c2-9df8-152478920bb7", op: TurnInput { request: TurnInputRequest { input: UserInput { content: [Text { text: "Generate a concise, single-line task title of at most 36 characters and under five words where possible. Start with an imperative verb. …
```

`connection_id=33` is the Codex UI connection proxied by `yinz hocus codex` for pane 4. The surviving rows for both threads
are in `title-thread-rebind.csv`: the real conversation `01a0ede6-…` and the title thread `01a0ede8-4a80-…`.
