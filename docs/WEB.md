# Native web listener

The web listener is disabled by default. It provides native password login, bearer
sessions, logout/revocation, HTTP health checks, frozen public pages/assets and
local `yinz web status` diagnostics. Authenticated application routes dispatch to
the configured web plugin. By default that is the builtin `web` plugin, which
serves the mobile agent app described in [the builtin mobile app](#builtin-mobile-app).
Native auth endpoints and rejected requests never dispatch to the plugin.

## Local startup and smoke test

From this checkout, install both its binary and builtin files into private
scratch directories. `--data-dir` installs the builtin Scheme plugins, including
`web.scm` and the app's frontend under `plugins/builtins/web/static/`, before the
binary. Do not copy a binary from another worktree or share `target/`.
The development profile avoids a release build for this local check:

```sh
umask 077
mkdir -p target/web-smoke /tmp/yinz-web-smoke-data /tmp/yinz-web-smoke-config
cargo x install target/web-smoke/yinz --profile dev --data-dir /tmp/yinz-web-smoke-data
./target/web-smoke/yinz --data-dir /tmp/yinz-web-smoke-data set-password
```

Enter and confirm a password of at least 12 Unicode characters. Input is hidden;
Ctrl-C cancels and restores the terminal. For automation use `--password-stdin`
with one UTF-8 line from a secret provider; do not put passwords in command
arguments. Input is limited to 1024 bytes, rejects NUL, and strips a final LF or
CRLF. Clear and revoke modes do not prompt or read stdin. The success message
suggests status for the same data directory; include `--config-dir` as below once
you create the smoke configuration. Recovery suggestions from status retain both
the selected data directory and explicit configuration options.

Write `/tmp/yinz-web-smoke-config/config.kdl`:

```kdl
web {
    enabled true
    address "127.0.0.1"
    port 7419
}
```

Start a fresh session in a terminal:

```sh
./target/web-smoke/yinz --data-dir /tmp/yinz-web-smoke-data --config-dir /tmp/yinz-web-smoke-config --session web-smoke
```

In another terminal:

```sh
curl -i http://127.0.0.1:7419/healthz
curl -i http://127.0.0.1:7419/
```

Health returns `200` JSON containing `"ok":true`; a direct loopback probe also
includes the current session name. Once the builtin app has published its pages,
`/` returns the app shell and `/login` the sign-in form. Readiness requires both
successful completion of the configured plugin's load hook (including deferred
work) and a nonempty publication from its selected live parent generation. Partial
publications are hidden. Until ready, public GET and HEAD return a generic `503`;
loading/retrying pages include `Retry-After` and an HTML refresh so an already-open
browser/PWA can recover without signing in. Terminal failure and permission-blocked
pages say **Yinz web is unavailable** and do not refresh endlessly. Internal errors
and paths never appear in these pages. The terminal session remains usable.
Frozen ready pages/assets support `HEAD`, strong ETags and conditional `304`.
Open `http://127.0.0.1:7419/` in a desktop browser to sign in with the password.

Every setting in `web { ... }` takes effect at the **next session start**, including
enabled/disabled, port, plugin and host/origin/identity allowlists. Reloading config
in a live session does not replace its listener or plugin parameters. Password
changes are live:

```sh
./target/web-smoke/yinz --data-dir /tmp/yinz-web-smoke-data set-password --revoke-sessions
./target/web-smoke/yinz --data-dir /tmp/yinz-web-smoke-data set-password --clear
```

Revocation retains the password hash and rotates its verifier ID. Setting a new
password rotates the salt, hash and ID. Both actions invalidate `sessions.json`;
every device is revoked across live servers and replacement sessions. Clearing
removes password and sessions files,
closes admission and releases enabled listeners within about one second. A new
valid password lets a waiting enabled session bind without restarting. All modes
operate on the local data directory, even if `--remote` was supplied. To provision
a remote machine, run this local command on that machine explicitly.

## Listener ownership and recovery

A secure password is required **before binding**, even for `/healthz`. Missing,
malformed, oversized, insecure or unsupported password files leave the listener
unbound. Use `set-password` to replace an invalid file; do not hand-edit a PHC
string or relax its permissions. The `web/` directory is private, updates lock a
persistent `.lock`, and password files are atomically published at mode 0600.
Never remove `.lock` while any process uses the directory. Clear never follows a
password/session symlink.

Each bound session writes its own private
`web/listeners/<pid>-<random-number>.json`, version 1, containing PID, unique
instance ID, effective socket address, current session name and optional immutable
startup configuration and optional application readiness (records are limited to
64 KiB). The readiness snapshot includes phase, target/load/generation, attempts,
retry eligibility and finite sanitized failure/ownership categories. It is updated
on transitions, without writing a changing countdown. Port 0 is supported
for tests: read the effective address from this record. The record follows session
rename and is removed after its listening socket closes. `yinz web status` probes
local `/healthz` to identify the responding session and binding instance. Only a
matching instance and session establish effective startup policy; reusable session
names alone cannot verify crash leftovers or legacy health responses. Direct
loopback GETs without forwarded or Tailscale headers include the public instance
identifier, which stays stable across rename and changes on a new binding. Status
distinguishes startup policy from current disk configuration. Legacy records
report unknown policy; records without application fields report readiness unknown.
Failed probes never prove staleness: cleanup requires a dead PID and a locked
identity recheck. Unsafe, malformed, replaced and ambiguous records are preserved.
Cleanup failures are logged
and do not hold the port. Failure to safely publish a record releases the listener
and retries; it never reports the listener as serving.

The configured publisher is reserved before initialization, including headless
restoration during live update. A bound listener's regular poll drives recovery;
public navigation does not dispatch application API requests. After an initial
initialization/publication failure, the host permits at most three automatic
retries, delayed 1, 5 and 30 seconds after each preceding failure. A load that
never publishes has a 60-second active readiness deadline. Waiting for a permission
decision for the selected publisher blocks recovery without cancelling its prompt;
explicit denial for that publisher and invalid plugin declarations are terminal.
A permission prompt or denial belonging only to another client’s nonselected
plugin instance leaves the working web app available. Update pause freezes
deadline, backoff and stability clocks. Stop/shutdown suppress further starts.

Brief success does not replenish the retry budget: the app must stay ready for 60
active seconds. Explicit configured-plugin reload/restart resets it immediately.
Exhaustion requires local correction and reload; a permanently broken installation
is not promised eventual recovery. Existing browser sign-ins and push subscriptions
remain native state and survive guest recovery. After correcting missing builtin
files, permissions or plugin code, the existing workaround is:

```sh
yinz --data-dir PATH --config CONFIG --session SESSION plugins reload web
# Or retain --config-dir DIR when that selects your configuration.
```

Use the effective configured plugin instead of `web` for a custom app. Keep your
original data/config selectors so the command reaches the intended installation.
An intentionally stopped app stays stopped: stop removes its loaded identity, so
use `plugins start` rather than `plugins reload` to bring it back:

```sh
yinz --data-dir PATH --config CONFIG --session SESSION plugins start web
```

Retain `--config-dir DIR` if you used it, and substitute the effective configured
alias/location for a custom app. Status provides `start` guidance for a verified
stopped app and `reload` guidance for a retained failed app, beside that listener's
session. Replace its SESSION/PLUGIN placeholders before running the command.
Legacy or unverified records do not supply a recovery action.

Changing the startup-only `web` block requires a new session, not just a plugin
reload.

An occupied port leaves an eligible session waiting. It retries after three
seconds plus up to one second of jitter; other bind/publication errors back off
longer. When the holder exits or quiesces for an update, it first releases the
socket, then drains admitted connections in the background for at most two
seconds. Another eligible session can take over. Old keep-alive connections cannot
resume admission after password loss, quiesce or shutdown. This offers no defense
against another local user binding the port first; use a trusted single-user host
and verify the process behind the port when configuring a proxy.

## HTTP policy and optional tailnet access

Bind to loopback unless non-loopback exposure was explicitly configured. Requests
must have a single valid allowed Host: loopback, a `.ts.net` name, or an exact
`allowed_hosts` entry. POSTs require an exact permitted Origin (including port),
JSON content type and acceptable fetch metadata, including on health/static/login
paths. Forwarded host/identity fields are trusted only from a loopback transport
peer; they never bypass Host validation. `tailscale_users` is an additional identity
restriction, not a replacement for authentication. A direct loopback GET health
probe without forwarded/Tailscale headers bypasses only this identity restriction
so local status can identify the listener; Host and Funnel checks still apply.
Funnel-marked requests are
rejected. No CORS or HSTS headers are added.

For optional tailnet-only access, configure Tailscale Serve to forward HTTPS on a
chosen port (for example 7443) to `http://127.0.0.1:7419`. Consult the installed
`tailscale serve --help` for its current command syntax. Do not enable Funnel.
Before relying on this deployment, verify Serve's actual Host, Origin,
`X-Forwarded-Host`, `X-Forwarded-For`, `Tailscale-User-Login` and Funnel-marker
behavior against this native policy. Live proxy/phone verification has not been
performed: the builtin app was exercised only by desktop-browser emulation
(see [testing the builtin app](#testing-the-builtin-app)).

Yinz-created responses receive CSP, nosniff, no-referrer, DENY, COOP, CORP and
Permissions-Policy headers. Native/API responses use no-store; frozen files use
no-cache. API responses vary on Authorization. Hyper itself may generate empty,
connection-closing `400`/`414`/`431` parser rejections before the service runs;
those transport responses may lack the Yinz headers and never dispatch or serve
content. Idle/header deadlines close the connection; bodies have a ten-second
deadline, size limits, strict UTF-8 and connection-close handling on partial reads.

## Bearer sessions and native endpoints

All endpoints use JSON and `Cache-Control: no-store`. POSTs must pass the same
Origin, fetch metadata and JSON content-type policy as other routes. Login bodies
are limited to 4096 UTF-8 bytes:

```json
{"password":"from your secret provider","label":"My phone","remember":false}
```

`POST /api/login` returns `200 {"token":"yzw1_...","session":{...}}`. Keep the
returned token secret and send exactly one `Authorization: Bearer TOKEN` header.
Cookies and query parameters do not authenticate. `GET /api/session` returns the
public session ID, label, creation, last-seen, absolute and idle expiry timestamps
in Unix milliseconds. `POST /api/logout` and `POST /api/logout-all` take `{}` and
return an empty `204`; logout-all rotates the verifier ID and revokes every device.
Wrong native methods return `405` with `Allow`.

Login rejects malformed JSON, unknown/duplicate fields, wrong types, NUL and
passwords over 1024 UTF-8 bytes with `400 invalid_request`. Labels are at most
128 bytes, with whitespace collapsed and remaining controls rejected; absent or
blank labels use a sanitized, capped User-Agent or `Browser`. Wrong passwords
return `401 invalid_credentials`. Missing/malformed/revoked bearer tokens return
`401 unauthenticated` and `WWW-Authenticate: Bearer`. A missing/invalid password
returns `503 no_password`. Unsafe, corrupt or unsupported session stores fail
closed with `503 session_store_unavailable` and are not overwritten by login.
Recover with `set-password --revoke-sessions`, setting a new password, or clearing.
Policy, body and transport failures keep their `400/403/408/413/415` codes.

Random 256-bit tokens are never stored: `web/sessions.json` holds SHA-256 hashes
and independent public IDs. It is owned, private (0600), nonsymlink, at most
256 KiB and atomically updated under the persistent password lock. At most sixteen
sessions survive; login evicts the oldest last-seen session with a stable ID tie
break. Absolute expiry uses `session_days` (default 30). Idle expiry is twelve
hours without remember and fourteen days with remember, capped by absolute expiry.
Equality at either deadline expires the session. Authentication persists advancing
last-seen timestamps under the same lock used by session mutations and push expiry
checks before accepting activity. Independent processes therefore see renewed idle
deadlines when checking or pruning push subscriptions. This writes the session
snapshot for each advancing touch; write failures reject authentication. Touches
cannot restore externally revoked rows.
Password rotation/revocation works across independent server caches and takeover.

Argon2id runs off the async executor: one active verification and two waiters;
queue overflow returns `429` with positive `Retry-After`. Five completed wrong
passwords per client in fifteen minutes cause a sixty-second lockout, doubling
on repeated lockouts up to fifteen minutes. Success resets that client's history;
escalation expires after fifteen minutes without failure. Thirty global failures
per hour lock new logins for fifteen minutes, deliberately trading login
availability for brute-force resistance. Existing sessions remain usable. Client
keys use a single bounded Serve identity only from loopback, otherwise peer IP;
X-Forwarded-For is not trusted for limiter identity. Missing identity (including
Funnel/tagged clients) shares the proxy's peer bucket. Local processes are trusted
when using a local proxy. Client history is bounded to 1024 keys plus a conservative
overflow bucket. API sessions get 20 requests/second, burst 40; public `/`, `/login`
and `/static/**` share 50/second, burst 100, including HEAD/304/starting replies.
Health is excluded. Every limiter denial returns `429` and positive `Retry-After`.
Passwords, bearer tokens/hashes, Authorization and native auth bodies never enter
plugin requests or logs. Plugins receive only the public session ID and label.

## Builtin mobile app

The default web plugin is the builtin `web` plugin (`yinz:web`, through the
default `web` alias with `shared true`). Only the session that serves
`web { enabled true }` loads it, headless. Builtins already hold `RunCommands`,
so it publishes without a permission prompt. Its `load` calls
`(yinz-web-agent-app configuration)`, which registers the routes and render
hooks, freezes `builtin:web/static` under `/static/` and publishes `/` and
`/login`. The route and hook contract is in
[the SDK reference](PLUGIN_SDK.md#standard-agent-app); replace the plugin with
`web { plugin "…" }` to customize it.

The only plugin configuration the app reads is `title`: 1–64 characters, not
blank and without control characters. Anything else keeps the default `yinz`.
The alias in your configuration replaces the default alias, so keep
`shared true`:

```kdl
plugins {
    web location="yinz:web" {
        shared true
        title "Agents"
    }
}
```

Each page embeds a public bootstrap object with the title, poll intervals
(list 3 s, timeline 2 s, terminal 250 ms, usage 60 s), the list's quick keys and
request timeouts (10 s for reads, 45 s for actions). It never contains other
plugin configuration or secrets.

### What the app offers

- **Agents** (`#/`) shows live agents from the serving session and its cached peer
  sessions, grouped by their pane's Yinz tab group. Each heading shows the group
  name, agent count, and owning session, including when only one group has
  agents. If headings in one session share a group name, each matching group
  also shows its stable group ID. If distinct session names differ only in
  whitespace, their headings preserve the original spelling and show an
  ordinal such as **1 of 2**. Sections appear in the order their first
  agent appears in the list returned by the hooks,
  and cards in each section keep their relative order. A card without usable
  group information appears under **Tab group unavailable** for its session.
  If the owning session is also missing, the heading says **Owning session unknown**.
  Stored membership still applies when `tab_groups` is off in the terminal.
  Local moves and renames appear on the next successful list update, and peer
  changes appear after the serving session's peer cache refreshes. For peers,
  headings and session badges use the scanned socket name, even while the cached
  checkpoint still contains an older name. Each card
  shows its state and any custom status text. The header's attention count
  includes all waiting agents in the snapshot, even when a hook hides some
  cards. Selecting the count focuses and reveals the next waiting card with
  an Open link in displayed order, wrapping back to the first. When multiple
  waiting cards are reachable, press Tab from a card's main Open link to reach
  its **Next waiting agent** button. Enter on the focused Open link opens the card.
  Task titles use the theme's accent color and
  replies use the main text color. Default reply previews retain up to 160 Unicode
  characters, with an ellipsis for locally or upstream truncated text, and display
  at most three lines. Blank replies take no space. Open the agent's timeline and
  message reader for longer content. Badges mark peer sessions, the
  desktop-focused agent and remote hosts. Waiting cards have quick keys and an
  **Open** button. The header has icons for **Switch group** and **Refresh**. The menu has **Log out** and **Log out everywhere**, which
  asks for confirmation.
- The **+** button in the Agents header opens **Start agent**, even when the list is empty. Choose a provider,
  a registered project, and a checkout by branch and path. A detached checkout
  appears as **Detached HEAD**. **Create new worktree** asks for a branch and
  uses the same directory and base as the command palette. Yinz places the pane
  in the serving session. The Agents page waits up to 60 seconds for the provider
  to report a conversation. It checks the launch receipt and live agent identity
  before opening the conversation. Choose **Dismiss** or leave the page to stop waiting.
  If the response is lost, the wait expires, or the server or worker changes,
  reopen **Start agent** in the same browser tab. **Check launch** uses the
  saved submission to recover the outcome and can start another 60-second wait.
  If the outcome stays unknown after a restart, check the agent list before
  starting another agent. The original may already be running. **Forget launch**
  asks for confirmation before clearing the saved submission and refreshing
  the choices. **Cancel** keeps it available.
- In a serving-session agent's **More actions** menu, **Move agent** shows its
  location and lets you choose a group and tab by name and ID. You can also
  create a tab in an existing group, or create a group and move into its first
  tab. Leave names blank to use the Yinz defaults. The pane and conversation
  stay together and the detail view stays open. **Close agent** shows the agent
  and location for confirmation, closes the pane, and returns to Agents. Its
  recorded history stays available. Yinz refuses a close that would also close other panes. Peer agents retain their existing controls.
- **Agent detail** (`#/a/<id>?run=<run>&tab=timeline|term|details`): a compact
  title and labelled live state, Back, **Agent information** (ⓘ), available
  actions and three tabs. Agent information starts collapsed on phones and
  desktop. Open it for the full title, complete server header (including custom
  hook output), host/session/focus badges and provider quota or usage errors.
  Peer links identify the conversation in the running peer. A rename or a
  newly recorded run keeps that binding. A restart, conversation change or older
  bookmark can leave only the retained run available. Return to Agents and
  reopen the card for the current binding. Older peers without a published
  identity open directly to the stored Timeline when a run is available. When
  there is no recorded run, the card explains why live controls are unavailable.
  This labelled region scrolls when its content is long. Its open/closed choice
  survives polling, tab changes and a live agent moving to a new run; leaving
  or reloading the detail view resets it. Expanding it preserves the draft and
  reading position. A live waiting agent opens on **Term**. A stored run opens
  on **Timeline**.
  - **Timeline** keeps the newest message at the bottom unless you scroll up,
    in which case a **New messages** button appears. **Load earlier** follows
    the native `earlier` navigation. Tapping a message opens its reader; tool
    groups open the tools reader, which pages through older records.
  - **Term** opens an interactive xterm.js view of the live pane. Tap the
    terminal to type or paste, or use the quick keys below it. The grid scales
    to the available width and keeps the pane's original columns and rows.
    When the grid is taller than the panel, Term opens at the bottom. Scroll
    up to keep an earlier row in view, or return to the bottom to follow the prompt.
    Opening an older `tab=screen` bookmark also selects Term. Scrolling up in
    the desktop pane pauses Term and its input until the pane returns to the
    bottom and processes its buffered output. Large snapshots load in parts.
    Typing and quick keys become available after the final part has rendered.
    A failed poll pauses queued input until the terminal has caught up.
    If a write fails, check the terminal for
    partial input and use **Reconnect** to resume typing.
  - **Details** shows run details with **Previous run**, **Original run** and
    **Latest**. Previous and original runs open as stored runs, marked by
    `&bind=0` in the hash: they are read without the live agent, show a
    "stored run" banner and offer no actions.
- **Composer**: a prompt box with a **Past prompts** sheet. Prompting is hidden
  when no live agent is bound and disabled with the native reason when
  `can_prompt` is false.
  When a scoped peer is unavailable, its exact Detail link still offers its
  saved text for copying or discarding, including after reload when the first
  History or Agents request fails. A separate edit and a frozen attempt can be
  discarded independently. If an explicit retry cannot verify the peer, reopen
  the exact link once the peer returns to retry with the original submission ID. Drafts
  saved before peer identity was recorded stay in browser storage without
  transferring to a peer composer.
- **Actions sheet**: **Focus on desktop** (labeled with the target session for
  peers), **Interrupt**, which sends `Esc` after you confirm, and **Send keys…**
  for up to eight of the native keys. Native code exposes no restart, close or
  spawn action.

Peer Term, keys, prompts and **Focus on desktop** use the scanned session
socket and check the running Screen instance, pane, provider and conversation.
Focus also checks that the pane is selectable. It verifies the peer before
asking the desktop to reconnect by session name. The peer can still change
between verification and that reconnect. During a rename, Focus becomes
available once the peer cache contains the newly published session name.
A Focus-only failure explains the reason and leaves other live controls
available while the binding is valid. If a
key reports that the agent changed or disappeared, refresh Agents and reopen
its card. If delivery is uncertain, inspect the agent before retrying.

There is no search UI. Archived-agent search and resume are not part of this
app.

### Sign-in, storage and logout

`/login` posts `{password,label,remember}` to the native `/api/login` with
`fetch`. With **Remember this device**, the token is stored in `localStorage`
under `yinz.web.v1.token`; otherwise in `sessionStorage`, so it lasts for the
tab. Signing in removes the token from the other store. If `localStorage`
refuses the token, the app falls back to `sessionStorage` and says that the
device will not be remembered. If neither store works, the app revokes the new
session and asks you to allow site data. A `429` or `503` with `Retry-After`
disables **Sign in** and shows a countdown.

The token is sent only as an `Authorization: Bearer` header to same-origin
paths. It never appears in a URL, page markup or log. Any `401` clears the
token, saves the current hash (never credentials) and returns to `/login`;
after sign-in the hash is restored. When another tab removes a remembered
token, open tabs of the app also return to `/login`.

**Log out** and **Log out everywhere** call the native endpoints and clear the
local token even when the request fails. If revocation was not confirmed by an
empty `204`, the notice says so. All keys use the
`yinz.web.v1.` prefix.

### Slash commands

Enter `/command` or `/command arguments` in the composer and press **Send**.
Claude, Codex and Pi receive executable input through their interactive command
parser: the command name is typed and arguments are pasted, including multiline
arguments. Yinz does not maintain a command whitelist. Available commands and
argument syntax depend on the target provider and its installed commands.
Other providers currently reject web slash input with an unsupported message.
Ordinary messages continue to use prompt submission.

Commands require an idle agent with no outstanding submissions. The composer
returns to idle once Yinz delivers the input and submit key. Check **Term**
for unknown-command errors, argument validation,
mode changes or dialogs; use the key controls to answer dialogs. Commands such
as `/new`, `/clear` or conversation selectors may replace the conversation; the
app refreshes the target and never redirects a saved submission to a replacement.
A command does not wait for a normal prompt-completion hook. An uncertain retry
uses the same submission ID and inspects its retained receipt without executing
it again; nothing retries automatically.

### Prompt safety

Drafts and submissions are kept per agent identity (`id`, `kind`,
`session_id`) in `sessionStorage`. They survive a reload or a trip through
`/login` in the same tab, but not closing the tab, and they are not shared
between tabs. Every edit saves the draft. Before a prompt is sent, the app
stores the target, the frozen text and a new submission ID, then reads the
record back. If that write fails, **Send** is blocked and the draft stays on
screen for copying.

Only native `submitted` (prompt acceptance) or `command_sent` (command delivery)
for the same target and submission ID
clears the attempt, and the draft if it was not edited meanwhile. An HTTP `200`
is not success on its own: native outcomes such as `timed_out` or
`status_unavailable` arrive inside a `200`. Definite failures, such as
`rejected`, `prompt_delivery: "not_sent"` or a native policy rejection
(`400`, `403`, `404`, `408`, `413`, `415`, `429`), show the reason. Uncertain
outcomes (timeouts, network errors, other HTTP errors including `401` while
sending, unreadable replies, or a reload during sending) show "Outcome unknown — check the timeline" with
**Retry same submission**, which reuses the same ID and text, and **Discard**.
Nothing is resent automatically, including after sign-in, reload or reopening
the agent.

### Target changes

A `409`, a `target_*` outcome before delivery started, a retargeted run or a
change of the pane's agent kind or conversation disables the affected record.
The prompt is never redirected to the replacement. You choose **Use the text as
a new draft**, which sends with a new submission ID, or **Discard**. Records
from an earlier conversation of the same pane appear above the composer with
the same choices. Keys and interrupts that get a `409` report that nothing was
sent and refresh the view; focus does the same for `target_changed`.

### Polling, staleness and Retry-After

Each resource has one poller with at most one request in flight; **Refresh**
joins it. Polling pauses while the page is hidden and refreshes on return.
Network errors, timeouts, unreadable replies and `429`/`502`/`503`/`504` back
off from 2 to 30 seconds, or follow `Retry-After` (at least one second, at
most ten minutes). Manual refresh waits out a pending `Retry-After`. Other
errors retry after 30 seconds. While a read is failing, the last data stays on
screen, dimmed, with a banner such as "Offline · updated 2m ago". Actions
(prompt, keys, focus) never retry automatically.

App routes return `503` with `Retry-After: 5` when native agent views are
`paused` (during an update) or `unavailable`. Native dispatch failures keep
their own `Retry-After` values.

### Page policy

The app has no build step and loads only same-origin module scripts under the
native CSP (`script-src 'self'`, `form-action 'none'`). It uses no inline
script, inline style, `eval` or import map. The bootstrap is an inert JSON
script. Server-rendered `*html` fields are inserted through a single component;
every other value is rendered as text. Preact 10.29.8 with its hooks and htm
3.1.1 and xterm.js 6.0.0 are vendored as separate modules in `web/static/vendor/`, with licenses
and provenance in `VENDOR.md`.

`/static/manifest.webmanifest` declares a stable identity, standalone display,
icons and theme color. The app includes Apple Home Screen metadata, safe-area
layout, and a service worker. When disconnected, navigation shows a reconnect
page; authenticated pages, agent output and API responses are never cached.

### Push notifications on iPhone and iPad

Open the HTTPS Serve URL in Safari, choose **Share → Add to Home Screen**, and
open Yinz from that icon. Sign in, then choose **Menu → Enable notifications**
and allow the system permission prompt. iOS/iPadOS 16.4 or later is required.
Other browsers with Web Push support can enable notifications from the same menu.
Permission is requested only after tapping Enable. The menu also disables pushes
for this device; logging out revokes delivery for that sign-in.

Every unsuppressed session notification is sent, including toast-only and desktop
notifications from detached sessions, across all local sessions using the same
data directory. Notification policies still apply, including replacements. Pushes
show the originating session, title and body. Tapping opens the agent list.
The app can be closed and the phone need not keep a polling connection open.
The originating Yinz session must remain running, and its server needs outbound
HTTPS access to Apple, Google, Mozilla or Windows push services. The HTTPS web
URL still needs to be reachable when opening the notification.

Signing keys and subscriptions persist privately in `<data-dir>/web/push.json`,
shared across local sessions and restarts. No external push relay or manual key
setup is needed. Subscriptions are bound to web sign-ins: expiration, revocation,
password rotation and Log out everywhere stop delivery. Expired browser endpoints
are removed after a 404/410 response. Delivery is best effort with a bounded queue;
messages expire at the push service after one hour. Device notification settings
and network availability can affect delivery. Actual Home Screen installation and
Apple push delivery require verification on a physical iOS device.

## Local status and optional Serve diagnostics

```sh
yinz --data-dir PATH web status --https-port 7443
```

This local command honors `--config` and `--config-dir`, loads no session layout,
prints labeled defaults and a warning if configuration cannot be read, and clears
inherited `--remote`. It identifies the inspected data directory; password recovery
and provisioning handoff commands retain that store and explicitly supplied
`--config`/`--config-dir`. Suggested local commands quote paths literally, including
spaces and apostrophes. Keep those options when following the custom-directory
walkthrough above. It reports configured values separately from probed live
addresses and recorded startup policy, plus password state/update time and public
active-session summaries. Port 0 remains configured port 0 while the listener
reports its actual bound port. No verified listener means runtime state is unknown,
not proof that a configured server is running. Multiple listeners are reported.
Listener liveness and application readiness are separate: health can return `200`
while `Application: retrying` or `Application: failed` explains the unavailable
app. Only instance-and-session-verified records supply readiness; unverified and
legacy records show unknown. Configured-plugin diagnostics also use bounded safe
failure summaries; detailed errors remain in the local log.

The HTTPS port is the override, then `https_port_hint`, then 7443. Suggestions use
a verified backend when available, otherwise an explicitly configured-only target:

```sh
tailscale serve --bg --https=7443 http://127.0.0.1:7419
```

Only bounded, read-only `tailscale status --json` and `tailscale serve status --json`
children run; they are killed/reaped on timeout or excess output. The expected
HTTPS origin uses `Self.DNSName` without its trailing dot. Background and foreground
`AllowFunnel` booleans are matched to Web Proxy backends on every reported port,
including local aliases, IPv6 and target paths. A matching public Funnel prints a
warning and exits 1. A same-port IPv4 loopback backend (including localhost)
targeting an IPv6 wildcard listener may also reach it through dual-stack IPv4.
Status warns that reachability is unverified and exits 1 for that possible
exposure; inspect or disable the mapping before relying on privacy. This
conservative warning also occurs on IPv6-only hosts because listener records do
not establish the socket's IPv6-only setting. Unrelated mappings do not fail.
Missing/error/unsupported schemas report exposure unknown. No-match output covers only the reported device
Serve configuration, never verifies all network exposure or configures Serve.

Verified references: [Serve CLI](https://tailscale.com/docs/reference/tailscale-cli/serve),
[Serve identity headers](https://tailscale.com/docs/features/tailscale-serve), and
[ServeConfig](https://pkg.go.dev/tailscale.com/ipn#ServeConfig). The installed
1.104.5 read-only diagnostics expose Self/DNSName and TCP/Web/AllowFunnel.
Identity headers are stripped/replaced by Serve, absent for Funnel/tagged clients.
These references do not establish every deployed Host/XFF/Funnel-marker behavior.
No live proxy or phone smoke check was performed; verify the actual headers and
browser Origin against your deployment before relying on it. The Funnel-marker
request guard is defense in depth, not proof of privacy when the marker is absent.

## Testing the builtin app

Each layer has its own selected tests. Mocked layers are named as such; results
are recorded in the implementation evidence, not here.

| Layer | Command | Real | Mocked |
| --- | --- | --- | --- |
| Scheme routes, validation, hooks and pages | `cargo x test -p yinz-plugin-host -E 'test(scheme_builtin_web_)'` | Builtin `web.scm` and SDK in a plugin worker | Every host command: `AgentView` replies, `HttpRespond`, `HttpAssets`, `HttpPublish` |
| Builtin loading | `cargo x test -p yinz-plugin-host -E 'test(migrated_builtins_load_through_the_real_host_dispatcher)'` | Host dispatcher publishing the installed `web` tree | No listener |
| SDK installation and JSON checks | `cargo x test -p yinz-steel -E 'test(/web/)'` | Steel runtime | Host command sink |
| Static tree | `cargo x test -p yinz-steel-builtins -E 'test(/web_tests::/)'` | Shipped `web/static` files: installation, served-file allowlist and limits, page and manifest references, import graph, vendor provenance and digests, the single HTML sink, CSP-safe templates | — |
| Retained builtin assets | `cargo x test -p yinz-runtime -E 'test(nested_static_assets_survive_exchange_without_unserved_records)'` | Nested `web/static` files across installation exchange | Temporary data directory |
| Frontend logic | `node --test yinz-steel-builtins/src/unit/web/{api,timeline,submission}.test.mjs` | `api.js`, `timeline.js`, `submission.js` | Injected fetch, storage, clocks and timers |
| Production HTTP | `cargo x integration-test -E 'test(web_builtin_production_http)' -j 16` | Listener, password and bearer auth, plugin host, worker and the default builtin from this checkout | Pane OS and processes (TestRunner's fake PTY) |
| Startup recovery | `cargo x integration-test -E 'test(web_startup_)' -j 16` | Production listener/pulse/host/publication, GET/login/HEAD recovery, finite budget, real 60-second unpublished deadline, native auth/subscriptions | Disposable builtin fault; fake PTY |
| Live update and rollback | `cargo x test -p yinz -E 'test(web_startup_live_update_)' -j 4` | Real source/destination binaries, headless inventory, paused restoration, pages/assets and existing browser identity | Disposable local installation |
| Custom app compatibility | `cargo x integration-test -- -E 'test(enabled_health_starting_rename_clear_and_terminal_usable) \| test(missing_invalid_password_then_write_binds_and_config_waits_for_next_start) \| test(web_transport_authenticated_steel_and_native_frozen_site) \| test(web_sdk_authenticated_routes_pages_assets_and_failures)' -j 16` | Existing auth, permission, publication, reload/stop and startup-only configuration callers | Disposable sessions; fake PTY |
| Deferred load readiness | `cargo x test -- -p yinz-steel -E 'test(web_startup_load_completion_waits_for_its_deferred_and_rejects_failure)'` | Successful and failed deferred load completion consumed by host readiness | Confined runtime |
| Browser flows | `web_builtin_browser_phone_flows`, see below | Headless Firefox via Playwright, frozen pages, native login | Agent-data API, intercepted with fixed fixtures |

Against the fake PTY, the production test records native behavior rather than
success paths: keys return HTTP `200` with outcome `status_unavailable` (the
process cannot be verified, so nothing is delivered); a prompt with a wrong
`session_id` returns `409 target_changed` before delivery; focus returns
`{"ok":true}` with a terminal attached and `409 unavailable` "No terminal
attached" after detaching.

The browser test needs a pinned, test-only Playwright package and a Firefox
build in this worktree's `target/`. Neither is shipped or installed with Yinz.
The test is ignored by default, so `cargo x ci check` does not run it; select it
with `--run-ignored only`:

```sh
npm ci --prefix yinz-steel-builtins/src/unit/web/browser --ignore-scripts --no-audit --no-fund
env PLAYWRIGHT_BROWSERS_PATH="$PWD/target/web-browser" \
  node yinz-steel-builtins/src/unit/web/browser/node_modules/playwright/cli.js install firefox
env PLAYWRIGHT_BROWSERS_PATH="$PWD/target/web-browser" \
  cargo x integration-test --run-ignored only -E 'test(web_builtin_browser_phone_flows)' -j 16
```

None of these tests uses a phone, Tailscale Serve or a real agent. Real phone
and Tailscale validation, and the cross-milestone takeover and revocation
suite, are tracked separately.
