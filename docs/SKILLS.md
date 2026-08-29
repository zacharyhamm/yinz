# Hocus skills

A skill is a prompt kept on disk and typed as a slash command. `/bench fix the flaky test`
sends the `bench` skill's text to the model as your turn, with `fix the flaky test` as its
arguments. That is all a skill is: a prompt, never a tool. Whatever hocus decides to do with it
goes through the same confirmation its own tool calls get — typing a skill is not typing
`/close_pane`.

## Where they live

Two folders are read, and only these two:

| tier | path |
|---|---|
| project | `<project root>/.hocus/skills/<name>/SKILL.md` — the active project's, read again whenever the active project changes |
| user | `<user plugin dir>/hocus/skills/<name>/SKILL.md` — `~/.config/zellij/plugins/hocus/skills/` by default; the KDL `skills_dir` (with `~/` expanded) points somewhere else |

A project skill shadows a user skill of the same name. A skill may not be named like a tool, one
of hocus's own commands (`/plan`, `/skills`, …) or an extension tool: the tool keeps the name and
`/skills` says the skill was refused.

Hocus is its own agent with its own tools, so it never reads another agent's skill folders —
`.claude/skills`, `.agents/skills` and the like. A skill written for Claude Code names tools hocus
does not have. Only the file format is borrowed, so there is nothing new to learn to write one.

## The file

```
.hocus/skills/bench/SKILL.md
```

```markdown
---
name: bench
description: Run the render benchmarks and compare against the saved baseline.
---

Run `cargo x bench -- --baseline main` in a new pane and report the deltas.
Focus on: $ARGUMENTS
```

- The **folder name** is the command. Lowercase letters, digits, `_` and `-`, starting with a
  letter or digit, at most 64 characters. A frontmatter `name` that disagrees is a warning in
  `/skills`; the folder wins.
- `description` is what `/skills` shows. Without one, the body's first line is used.
- The frontmatter is optional. `|` and `>` block scalars work; nested maps (`metadata:`) and
  keys hocus does not read (`allowed-tools`, `model`, `argument-hint`, …) are ignored, never
  errors. `allowed-tools` is **not** enforced.
- `$ARGUMENTS` anywhere in the body is replaced by the text typed after the command. A body
  without it gets that text appended after the prompt instead, so the arguments are always
  somewhere the model can see them.
- A body is at most 24 000 bytes and a folder holds at most 32 skills; anything over is listed
  under "not loaded" with the reason. A skill is re-sent on every later turn of the
  conversation, like a tool result, so a long one is a standing cost.

## What the model sees

```
<skill name="bench" path="/work/hocus/.hocus/skills/bench/SKILL.md">
Run `cargo x bench -- --baseline main` in a new pane and report the deltas.
Focus on: the flaky login test
</skill>
```

The tag marks the text as something nobody typed. The transcript shows the line you typed
(`/bench the flaky login test`), and so do the `queued:` notice, the chat's title and the archive.

A skill behaves as typed text everywhere else: it queues behind a reply that is still arriving,
an `@agent` in its arguments earns the usual context line, it answers a plan that is waiting for
one, and inside planning mode it is a planning turn. `/plan /bench …` is not a thing — `/plan`
takes the rest of the line as its prompt — but `/plan …` and then `/bench …` is.

## Commands

| command | does |
|---|---|
| `/<skill> [text]` | send the skill with `text` as its arguments |
| `/skills` | list what is loaded, with its tier, then what was refused and why |
| `/skills reload` | read both folders again and list the result |
| `/help` | the command list, with a `skills` row |
| Tab | completes a skill name like a tool's |

There is no file watcher: after editing a skill, `/skills reload`.

Reading the folders is a `sh` loop run on the host (`head -c` is used, so GNU, BSD or busybox
coreutils are all fine); a wasm plugin cannot open a file outside its own sandbox. A folder that
does not exist is simply empty. One that cannot be read leaves what was loaded in place and is
listed under "errors" in `/skills`.
