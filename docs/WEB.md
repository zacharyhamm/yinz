# Native web listener

The web listener is disabled by default. It provides native password login, bearer
sessions, logout/revocation, HTTP health checks, frozen public pages/assets and
local `yinz web status` diagnostics. The mobile UI, Steel HTTP bridge and agent
API are later milestones; authenticated application routes may return `503`.
Native auth endpoints and rejected requests never dispatch to the plugin.

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
startup configuration (records are limited to 64 KiB). Port 0 is supported
for tests: read the effective address from this record. The record follows session
rename and is removed after its listening socket closes. `yinz web status` probes
local `/healthz` to identify the responding session and binding instance. Only a
matching instance and session establish effective startup policy; reusable session
names alone cannot verify crash leftovers or legacy health responses. Direct
loopback GETs without forwarded or Tailscale headers include the public instance
identifier, which stays stable across rename and changes on a new binding. Status
distinguishes startup policy from current disk configuration. Legacy records
report unknown policy.
Failed probes never prove staleness: cleanup requires a dead PID and a locked
identity recheck. Unsafe, malformed, replaced and ambiguous records are preserved.
Cleanup failures are logged
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
performed as part of this milestone. Native login works, while the mobile agent UI is still deferred.

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
Equality at either deadline expires the session. Last-seen writes are merged at
most every five minutes and during mutations, so other processes can observe up
to five minutes of idle-time lag. Touches cannot restore externally revoked rows.
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
