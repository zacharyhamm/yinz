This document details the different components of the code base in plain English. The intention is to provide a high level description rather than a more detailed drill-down of each internal mechanism.

## Data access

The server constructs one host-only `DataAccess` provider and shares it with the plugin runtime
and session recorder. Plugin data and recordings use backend-neutral synchronous interfaces;
SQLite is the default and currently only backend. Its files, SQL migrations, transactions, FTS5
indexes, and connection settings stay inside the concrete SQLite implementations. CLI recording
commands construct the same default provider at their entrypoint.

## Screen (zellij-server/src/screen/)
The Screen is responsible for managing the relationship between the panes currently displayed on screen.
It is in charge of actions such as: 
  * Coordinating the resizing of panes
  * Creating new panes
  * Closing existing panes (and filling up their space by other panes)

## Terminal Pane (zellij-server/src/panes/terminal_pane.rs)
The TerminalPane represents a pane on screen that connects to a single pty (pseudo terminal) and (presumably) has one shell or other program running inside it.
The TerminalPane has two main roles:
  * Keeping track of the Scroll, which represents the buffer of lines in this terminal
  * Interpreting the ANSI/VT instructions coming from the pty in order to adjust the styling of characters, change the cursor position, etc.

### Scroll (zellij-server/src/panes/terminal_pane.rs and zellij-server/src/panes/grid.rs)
The Scroll holds the terminal buffer and is in charge of:
  * Keeping track of the viewport (which part of it we see) this can change when we scroll up/down
  * Keeping track of the cursor position
  * Controlling line-wrapping

### Terminal Character (zellij-server/src/panes/terminal_character.rs)
The `TerminalCharacter` represents a single character in the pane. It holds the char itself on the one hand, and an internal `CharacterStyles` struct representing the styling of this character.
This struct derives the `Copy` trait for performance reasons, because it is moved around quite a bit (eg. when line wrapping).

### How a terminal emulator draws characters?
The ANSI/VT instructions received from the pty include style instructions (eg. change foreground colour to red) as well as instructions to reposition the cursor (eg. go to line x/y) and to perform certain actions with the current buffer on-screen (eg. delete all lines after cursor).

When an instruction is received to change the character style, we know that all characters printed from now on should use that style. This is true until we move to a new line (with a newline instruction or a go to instruction) or until we receive a style reset code for this specific style. There are several kinds of reset codes that apply to different styles.

It's important to understand that these styles are ongoing relative to the current cursor position. It might be useful to imagine them as brushes: "pick up the red brush", "put down the bold brush", etc.

This is important to note because the styles themselves are saved only on characters that have already been printed. If we receive an instruction to change the style to blue, then print a (blue) character, then receive another instruction to move to a new line, print another (plain) character and then come back, the style would be reset. We would have to receive a new instruction to change the style to blue in order for the next character to be blue.

## Boundaries (zellij-server/src/ui/boundaries.rs)
The Boundaries refer to those lines that are drawn between terminal panes. A few things that happen here:
  * The Rect trait is here so that different panes can implement it, giving boundaries a generic way to calculate the size of the pane and draw boundaries around it.
  * Here we use the [unicode box drawing characters](https://en.wikipedia.org/wiki/Box-drawing_character) in order to draw the borders. There's some logic here about combining them together for all possible combinations of pane locations.

## Animation (zellij-utils/src/animation.rs, zellij-server/src/ui/damage.rs)
Anything on screen that moves over time — toast notifications sliding in and out, the bell flash easing off a pane frame, the plugin loading dots — is built from the same two pieces.

`zellij-utils/src/animation.rs` holds the pure part: an `Easing` curve, a `Tween` carrying one scalar from where it is to where it is going, and `blend` for interpolating a `PaletteColor`. The rule that makes it work is that **nothing in it reads the clock** — `now` is always a parameter. One frame therefore samples every live animation at a single instant, and a test drives an animation by handing in fabricated `Instant`s rather than by sleeping.

The frames themselves come from one shared clock: `BackgroundJob::StartAnimationClock` runs a `tokio::time::interval` that sends `ScreenInstruction::AnimationTick`. That tick advances *nothing*. It only asks for a render, and `Screen::render_to_clients` samples everything at the instant it composites. This is what makes the result frame-rate independent: a tick that arrives late, twice, or not at all costs a frame and never a position, so an animation always takes the wall-clock time it promised even when the screen thread is busy or renders are being held back.

`Screen::animation_clock_period` is the one place that decides what the clock does, derived from the animated state rather than from a registration list, so the clock cannot get stuck running over an idle session. It answers with a period rather than a yes/no, because the surfaces differ: continuous motion — a toast sliding, a bell fading — needs `FRAME_PERIOD` (~60fps), while the plugin loading dots are a four-character "still working" pulse that only changes every `DOT_PERIOD` (500ms). Holding the clock at 60fps for the dots would run the whole `render_to_clients` path sixty times a second for as long as any plugin is loading, which for a plugin that never finishes is forever. The aggregate is computed on the screen rather than requested by each surface so that a plugin load starting mid-slide cannot slow the clock underneath a moving toast.

Every tick asks for a frame, *including* the one that finds everything settled. That last frame is the one that actually paints the settled state: renders are debounced by `REPAINT_DELAY_MS`, so the frame a still-animating tick asks for is composited up to that long before the deadline it was aiming at. Without one more tick past the deadline, a dismissing toast would be left painted a sliver short of gone — and never reaped, since `reap_settled_notifications` only runs inside a frame — until unrelated output happened to repaint.

Anything drawn *over* a pane — currently just toasts — needs one more thing. A pane only emits cells its own grid thinks are dirty, so the moment a moving overlay stops covering a cell, that cell keeps whatever the overlay last drew there. `ui/damage.rs` diffs what the overlay painted last frame against what it is painting now, and `render_to_clients` repairs the difference *before* the panes render, narrowed to the panes actually overlapping it. An overlay that is only growing produces no damage at all, so arriving is free and only leaving costs repaints.

## PTY Bus (zellij-server/src/pty.rs)
The PtyBus keeps track of several asynchronous streams that read from pty sockets (eg. /dev/pts/999), parse those bytes into ANSI/VT events and send them on to the Screen so that they can be received in the relevant TerminalPane.

## Pane supervisors (zellij-server/src/pty_host/)
Each pane's process is owned by a small supervisor process ("pty-host") rather than by zellij-server. The supervisor calls `openpty`, forks the pane's command, and hands the server a dup of the pty *master* over `SCM_RIGHTS`. Everything downstream of that descriptor — the async reader, window-size ioctls, backpressure on writes — works exactly as it did when the server owned the pty.

What the split buys is lifetime. The child belongs to the supervisor, so a server that exits or is killed does not take its panes with it. The supervisor reads a closed control channel as "detached" rather than "shut down" and starts draining the pty into a bounded ring buffer, so the child never blocks on a full buffer. Only an explicit `Terminate` ends a supervisor.

Because a detached supervisor is still holding a live pane, it can be picked back up. It binds a named socket, and an `Attach` on that socket re-keys it to the attaching server's pane, hands the master over again, and replays whatever the ring caught in the meantime. Two things are built on that one primitive:

  * `zellij action warp-pane --to-session <name>` moves a pane between sessions. The source releases the supervisor without killing it, serializes the pane's screen, scrollback and agent record, and sends the other session an `AdoptPane`; the target attaches and replays them into a new pane. The agent's *history* is not in that payload and does not need to be: it lives in `agent_run` rows in the plugin data store, which is one file per machine rather than per session, so what has to travel is the run's id rather than the run. That rides on the record, as `AgentInfo.run_id` — see `docs/AGENT_HOOKS.md`. The losing session closes the run under a predicate naming itself as its owner, and the gaining session reopens it by id, so whichever write lands second the entries survive. Nothing else about the process changes — same pid, same working directory, same open files. A local child inherits its supervisor id as `ZELLIJ_PANE_REF`; because `Attach` re-keys the supervisor's current session and terminal id, an implicit `agent-report` can resolve that stable id after the move instead of using the stale per-session values in the child's environment.
  * `zellij action warp-tab --to-session <name>` moves a whole tab the same way, releasing every supervisor in it and sending an `AdoptTab`. What travels alongside the panes is a *layout* and each pane's agent record — run id and all, so each pane's history is picked up the same way — rather than their pane sizes, reconstructed from their geometry by the same code session resurrection uses — so the tab re-flows to fit a session whose terminal is a different size instead of arriving clipped. The tab's plugin panes are the one thing that does not survive intact: they have no process to hand over, so they ride along as `Run::Plugin` nodes and the receiving session starts them afresh in the same slots.
  * `zellij pty-host adopt <id>` does the same thing with no source session at all, which is how a pane that outlived a crashed server gets back on screen.

The controlling option is `detachable_panes`. With it off, panes are spawned in-process as before and neither operation is available.

A **remote pane** warps too, by the same shape but not the same primitive: its supervisor is on the far machine, behind the broker, so there is no local socket to attach to. What the broker offers instead is already there for reconnects — panes live in a registry keyed by a token, independent of any connection, and `Resume { pane_token, received }` re-attaches to one with a byte-exact replay. A warp is that same resume with a *different server* on the other end, each having built its own ssh forward. So the pane's address travels as `{host, pane_token, received}` (plus its `RunCommand`, which a resume does not carry and an `AttachAck` would have) in place of the supervisor socket, and the adopting session resumes through its own forward.

The one thing that had to be added is `Release`. The broker frees a pane when the old connection's blocking read returns, which is not a moment the releasing side can name — the adopting session's `Resume` races it and is refused as a double attach. `Release` makes the detach happen before the acknowledgement, exactly as `ServerToHostMsg::Release` does for a local supervisor, so the token can be handed over the instant the release returns. Recovering a remote pane stranded by a failed handoff is `zellij pty-host list --host <machine>` and `zellij pty-host adopt --host <machine> <token>`: the token only ever existed in the releasing server's memory and the broker's registry, so the broker is the only thing that can still answer.

A **Firecracker pane** is a remote pane whose SSH endpoint comes from a configured provider.
Zellij keeps the VM name on `RunCommand`, calls the provider's idempotent `ensure` operation for
each launch, then feeds the returned endpoint into the same remote broker path. VM ownership and
shutdown stay session-scoped, so these panes deliberately cannot warp between sessions. See
[`FIRECRACKER.md`](FIRECRACKER.md) for the provider contract and lifecycle.
