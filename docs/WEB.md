# Native web listener

The web listener is disabled by default. This milestone provides HTTP/1 health
checks, a native starting page, frozen public pages/assets, password-file
provisioning, and session-owned startup/shutdown. Bearer login, persistent browser
sessions, Steel HTTP dispatch, the agent API, the mobile application and operator
web-status commands are not implemented. Setting a password permits binding; it
does not make `/api/login` succeed. No unauthenticated request dispatches to Steel.

## Local startup and smoke test

From this checkout, install both its binary and builtin Scheme files into private
scratch directories. Do not copy a binary from another worktree or share `target/`.
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
CRLF. Clear and revoke modes do not prompt or read stdin.

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
includes the current session name. With no published web application, `/` returns
`503`, an escaped **Yinz web is starting** page and `Retry-After: 1`. The terminal
session remains usable even if the builtin web application is missing. Frozen
public pages/assets support `HEAD`, strong ETags and conditional `304` retrieval.

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
bearer sessions remain deferred. Clearing removes password and sessions files,
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
instance ID, effective socket address and current session name. Port 0 is supported
for tests: read the effective address from this record. The record follows session
rename and is removed after its listening socket closes. These records are not an
operator status API; stale-record scanning is deferred. Cleanup failures are logged
and do not hold the port. Failure to safely publish a record releases the listener
and retries; it never reports the listener as serving.

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
restriction, not a replacement for authentication. Funnel-marked requests are
rejected. No CORS or HSTS headers are added.

For optional tailnet-only access, configure Tailscale Serve to forward HTTPS on a
chosen port (for example 7443) to `http://127.0.0.1:7419`. Consult the installed
`tailscale serve --help` for its current command syntax. Do not enable Funnel.
Before relying on this deployment, verify Serve's actual Host, Origin,
`X-Forwarded-Host`, `X-Forwarded-For`, `Tailscale-User-Login` and Funnel-marker
behavior against this native policy. Live proxy/phone verification has not been
performed as part of this milestone. This is a health/starting-page smoke check;
there is no working mobile login or agent UI yet.

Yinz-created responses receive CSP, nosniff, no-referrer, DENY, COOP, CORP and
Permissions-Policy headers. Native/API responses use no-store; frozen files use
no-cache. API responses vary on Authorization. Hyper itself may generate empty,
connection-closing `400`/`414`/`431` parser rejections before the service runs;
those transport responses may lack the Yinz headers and never dispatch or serve
content. Idle/header deadlines close the connection; bodies have a ten-second
deadline, size limits, strict UTF-8 and connection-close handling on partial reads.
