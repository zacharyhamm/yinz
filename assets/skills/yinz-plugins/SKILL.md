---
name: yinz-plugins
description: Author, check, test, reload, and diagnose Yinz Steel Scheme plugins using bundled SDK documentation and the installed Yinz CLI.
---

Use `yinz plugins docs index`, then `docs guide`, `docs sdk`, or `docs testing`.
These commands return JSON with bundled documentation in `content`; no source
checkout or internet access is required. Use `yinz plugins dir` and `plugins list`
to locate source files, then native file reading, search, and editing tools.
Follow the bundled SDK; plugins use confined `require` imports and the built-in Scheme SDK.

```sh
yinz plugins write clock.scm --file clock.scm
yinz plugins check clock.scm --json
yinz plugins test clock.scm --json
yinz --session main plugins start clock.scm
yinz --session main plugins reload clock.scm
yinz --session main plugins list --running --json
yinz plugins log clock.scm --lines 80
```

`write` formats and checks before publishing. Existing source requires
`--overwrite`; a backup is retained. Source input can be `--file -`. Prefer
checked edits when matching a specific old fragment:

```sh
yinz plugins edit clock.scm --file changes.json
```

The edit file is an array of `{old_string,new_string,replace_all?}` objects.
Each replacement must match exactly once unless `replace_all` is explicit.
The batch is checked and formatted before writing. Missing or ambiguous matches,
compiler errors, and stale source during checking leave the original intact.

Use `plugins start TARGET` to load a background plugin. Use `plugins reload TARGET`
to reload an existing identity in place, or `plugins stop TARGET` to unload it.
Targets accept names, aliases, paths, URLs, and `plugin_N`. Select an exact
configuration with `--configuration-json '{"shared":"true"}'` when needed.
No Agent plugin is required. `reload --headless` remains a compatibility spelling.
Reloading a missing target fails with `not loaded; use plugins start`.

Permission requests appear in a host-owned modal. Loading and permission requests
preserve pane focus and background placement. Start and reload report scheduling.
Use `plugins list --running --json` to inspect loading, pending permissions, or
failure. `plugins log` reports runtime status and labels historical log output.

`plugins check --json` emits structured diagnostics even on failure. Always check
process status. Use `plugins test --help` for test selection and bounded timeouts.
Reload discards in-memory state. Verify persisted data and subscriptions when relevant.
