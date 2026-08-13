This document details the different components of the code base in plain English. The intention is to provide a high level description rather than a more detailed drill-down of each internal mechanism.

## Screen (zellij-server/src/screen.rs)
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

## PTY Bus (zellij-server/src/pty.rs)
The PtyBus keeps track of several asynchronous streams that read from pty sockets (eg. /dev/pts/999), parse those bytes into ANSI/VT events and send them on to the Screen so that they can be received in the relevant TerminalPane.

## Pane supervisors (zellij-server/src/pty_host/)
Each pane's process is owned by a small supervisor process ("pty-host") rather than by zellij-server. The supervisor calls `openpty`, forks the pane's command, and hands the server a dup of the pty *master* over `SCM_RIGHTS`. Everything downstream of that descriptor — the async reader, window-size ioctls, backpressure on writes — works exactly as it did when the server owned the pty.

What the split buys is lifetime. The child belongs to the supervisor, so a server that exits or is killed does not take its panes with it. The supervisor reads a closed control channel as "detached" rather than "shut down" and starts draining the pty into a bounded ring buffer, so the child never blocks on a full buffer. Only an explicit `Terminate` ends a supervisor.

Because a detached supervisor is still holding a live pane, it can be picked back up. It binds a named socket, and an `Attach` on that socket re-keys it to the attaching server's pane, hands the master over again, and replays whatever the ring caught in the meantime. Two things are built on that one primitive:

  * `zellij action warp-pane --to-session <name>` moves a pane between sessions. The source releases the supervisor without killing it, serializes the pane's screen and scrollback as an ANSI dump, and sends the other session an `AdoptPane`; the target attaches and replays the dump into a new pane. Nothing else about the process changes — same pid, same working directory, same open files.
  * `zellij action warp-tab --to-session <name>` moves a whole tab the same way, releasing every supervisor in it and sending an `AdoptTab`. What travels alongside the panes is a *layout* rather than their pane sizes, reconstructed from their geometry by the same code session resurrection uses — so the tab re-flows to fit a session whose terminal is a different size instead of arriving clipped. The tab's plugin panes are the one thing that does not survive intact: they have no process to hand over, so they ride along as `Run::Plugin` nodes and the receiving session starts them afresh in the same slots.
  * `zellij pty-host adopt <id>` does the same thing with no source session at all, which is how a pane that outlived a crashed server gets back on screen.

The controlling option is `detachable_panes`. With it off, panes are spawned in-process as before and neither operation is available.
