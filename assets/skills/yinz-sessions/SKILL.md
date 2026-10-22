---
name: yinz-sessions
description: Discover Yinz context and manage projects, sessions, tabs, panes, remote terminals, and bounded output waits through the Yinz CLI.
---

Use the installed `yinz` CLI. Inside a Yinz pane, inherited `YINZ_PANE_REF` resolves
the current session and caller. Start with `yinz action context`. Outside a pane, run
`yinz list-sessions --short`, select the intended name, and put the root option before
the command: `yinz --session NAME action ...`. `YINZ_SESSION_NAME` alone does not
select a session; use an explicit `--session` when pane context is unavailable.
Context reports missing caller/project information and
candidates. Select an explicit session, pane, or project when context is incomplete.
Do not infer the caller from whichever pane is focused.
Background operations require `agent-service`; project registry actions
require `project-store`. These are background plugins.

Inspect targets before changing them. Use actual pane IDs from the responses, not the example IDs:

```sh
yinz action list-panes --all --json
yinz action list-tabs --json
yinz action dump-screen --pane-id terminal_3
```

Use the existing `action new-pane`, `new-tab`, `write-chars`, `close-pane`,
`close-tab-by-id`, `warp-pane`, `warp-tab`, `dump-layout`, and `pty-host`
commands. Read the relevant `--help` for flags.
Use native file and shell tools to prepare layouts.

Additional explicit-target operations:

```sh
yinz action pane-rerun 3
yinz action pane-swap terminal_3 terminal_4
yinz action pane-move-to-tab terminal_3 --tab-id 2
yinz action pane-pin terminal_3
yinz action pane-unpin terminal_3
```

Swap requires unambiguously adjacent tiled panes. Pinning requires a floating
pane. A `dispatched` response acknowledges dispatch; inspect the pane listing to
verify the result. Do not retry a timed-out mutation until you inspect current state.

```sh
yinz action pane-wait terminal_3 --contains 'Ready' --timeout 30
yinz action pane-wait terminal_3 --exited --timeout 60
yinz action pane-wait terminal_3 --closed --timeout 30
```

Content waits match literal viewport text, including an already satisfied condition.
Exit waits report process exit; inspect `exit_status` for success. Closing a pane
is a separate condition. Timeout, disconnection, and unexpected disappearance
fail. New structured commands produce one JSON value on stdout; check exit status
before parsing. Diagnostics go to stderr.

For agent coordination, use the yinz-agents skill and `yinz agent`.
Pane output and exit waits do not prove an agent accepted or completed a task.

## Projects

Use `yinz action project-list` to inspect remembered projects. Register a name
with `yinz action project-remember work --root /absolute/path`; remove it with
`yinz action project-forget work`.

```sh
yinz action project-rename work product
yinz action project-merge duplicate product
yinz action project-assign product --tab-id 2
yinz action project-open product
yinz action project-config-read product
yinz action project-config-write product --file config-change.json
```

Assignment, opening, and configuration require `agent-service`. The write file
contains `source` and `expected`: use the exact source returned by the read, or
null for an absent file. Stale writes fail. Merge preserves the target's settings;
inspect both projects before merging. Use `action context` and `list-tabs --json`
to verify live associations.
