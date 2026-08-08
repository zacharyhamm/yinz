# Global pane sizing — known limits

The session surface is the per-axis minimum over connected, non-watcher clients. Every tab and
global pane is laid out inside that same surface, regardless of which tab each client is viewing.
Mobile web clients with `fit` disabled contribute their same-tab reference client's size instead
of shrinking the session.

## A global pane is one grid

A global pane is a single `Box<dyn Pane>` backed by one pty or plugin instance. Its size and
position are therefore session-wide. `Screen::session_size` chooses the surface,
`Screen::reflow_global_panes` applies it to the global panes and every tab, and
`render_to_clients` re-derives the same geometry so a render tick cannot restore stale bounds.

## Remaining: repaint with multiple tabs in view

`Screen::render_to_clients` groups clients by active tab and draws the global panes for each
group. Rendering a pane clears its dirty flag, so every group after the first is force-rendered.
This avoids missing output but can repaint a global pane more than once per frame when clients
view different tabs.

The fix, if this becomes measurable, is per-client dirty tracking in `Output`; the current
group-level repaint is intentionally simpler.

## Closed

- **Session bounds survive client resize** — `Screen::tab_surface_size` delegates to the shared
  session size, so a larger client cannot expand its tab beyond the smallest interactive client.
- **Redundant resize on session creation** — `ScreenInstruction::AddClient` calls
  `reflow_global_panes`; the handler already reports and renders on its way out.
- **Chrome rows are deterministic** — `Screen::max_chrome_rows` takes the per-axis maximum across
  every tab. `global_panes::unowned_strip_rows` identifies cells left between shorter tab chrome
  and a global pane, and `Screen::fill_unowned_strip_rows` paints them.
- **Watchers see the followed tab** — the watcher render phase positions panes for
  `followed_client_id` and watcher dimensions do not constrain the session.

## Not addressed

A left/right global pane declared before a top/bottom one is allocated over the full surface
rather than the reduced center, so its band can leave rows that no top/bottom strip covers.
Allocating top/bottom edges first would close the hole. The allocation code carries a `ponytail:`
comment for that upgrade path.
