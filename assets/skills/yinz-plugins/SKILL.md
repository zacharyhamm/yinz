---
name: yinz-plugins
description: Author, check, test, reload, and diagnose Yinz TypeScript plugins using bundled SDK documentation and the installed Yinz CLI.
---

Use `yinz plugins docs index`, then `docs guide`, `docs sdk`, or `docs testing`.
These commands return JSON with bundled documentation in `content`; no source
checkout or internet access is required. Use `yinz plugins dir` and `plugins list`
to locate source files, then native file reading, search, and editing tools.
Follow the bundled SDK; plugins use confined relative imports and `yinz:plugin`.

```sh
yinz plugins write clock.ts --file clock.ts
yinz plugins check clock.ts --json
yinz plugins test clock.ts --json
yinz --session main plugins reload clock.ts
yinz plugins log clock.ts --lines 80
```

`write` formats and checks before publishing. Existing source requires
`--overwrite`; a backup is retained. Source input can be `--file -`. Prefer
checked edits when matching a specific old fragment:

```sh
yinz plugins edit clock.ts --file changes.json
```

The edit file is an array of `{old_string,new_string,replace_all?}` objects.
Each replacement must match exactly once unless `replace_all` is explicit.
The batch is checked and formatted before writing. Missing or ambiguous matches,
compiler errors, and stale source during checking leave the original intact.

For background plugins, use `plugins reload NAME --headless`; this requires
`hocus-agentd`. Its JSON result reports dispatch. Inspect logs and exercise the
plugin to verify that loading completed. `plugins check --json` emits structured
diagnostics even on failure; always check process status. Use `plugins test --help`
for unit/integration selection and bounded timeouts. A plugin reload can discard
in-memory state; verify persisted data and subscriptions when relevant.
