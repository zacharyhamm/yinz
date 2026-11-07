# Mobile agents web interface: final implementation plan

> Decided with the user: the HTTP server uses hyper directly (vetis was evaluated and rejected for idle-connection, reset and Host-routing bugs).

All paths are relative to `/home/zacharyhamm/yinz/.worktrees/mobile-agent-tracker`. I checked every file and line reference below against the code during this design pass. Items still marked **VERIFY** depend on outside behaviour (mainly Tailscale and the browser) or must be measured.

The plan starts from the **parity-reuse** proposal, because it reuses the most native palette code. It fixes every judge-verified error and adds the strongest ideas from **security-ops** (auth model, Funnel guard, listener state machine, frozen assets) and **steel-first** (`retained_source` fix, `open_beneath`, Steel-only commands, the plugin never invoked unauthenticated).

---

## 0. Decisions that differ from the proposals, and why

| # | Decision | Reason (verified) |
|---|---|---|
| D1 | **The session is a bearer token kept in `localStorage`/`sessionStorage` and sent as `Authorization: Bearer`. No cookies.** | Funnel already serves `https://<box>.ts.net:443` and `:8443`. Cookies are not scoped by port, so a cookie set for `:7443` would also be sent to those public services. Same-host pages on other ports also count as *same-site*, so `SameSite=Strict` does not stop them from forging requests, and they could plant (toss) a cookie of the same name. Browser storage is isolated by full origin, port included. Bearer headers are never sent automatically, which removes CSRF as a class of attack. |
| D2 | **Static assets and the two public HTML pages (`/`, `/login`) are frozen into a per-session in-memory `Site` when the plugin registers them. Native code serves them. The plugin is never called for an unauthenticated request.** | Builtins load from a frozen or snapshotted generation (`yinz-steel/src/modules.rs:116-131`, `yinz-runtime/src/builtin_generation.rs:130-197`). Reading files per request is TOCTOU-prone and can serve another generation's files. It also stops anonymous tailnet traffic from waking the Steel worker. |
| D3 | **Extend `builtin_generation::retained_source` so the snapshot keeps web assets.** | `retained_source` (`builtin_generation.rs:237-243`) keeps only `scm/ss/sld`. After an install exchange, `source_path` points into that snapshot, so without this fix any web plugin that loads or reloads after an update would find no assets. |
| D4 | **The new host commands are Steel-only, so there is no protobuf work.** | This follows the `CommandPalette` precedent: `yinz-plugin-api/src/plugin_command.rs:3730` returns `Err("Command palettes are Deno-only")`, and `unit/plugin_command_tests.rs:629` returns `None`. |
| D5 | **Pending reply slots live on the instance's `PluginEnv` (`web_pending: Arc<Mutex<WebPending>>`) and are cleared on dispose.** | Generation isolation comes for free: only the instance that received a request holds its slot. Waiting requests get 503 at once on dispose instead of timing out at 504. Requests are correlated without any process-global state. |
| D6 | **`AgentView` is sent through the plugin thread (`PluginInstruction::Web(AgentView)`) and spawned there with `native_tasks.start()`, after checking `update_pause`.** | This matches the `PaletteAgent` arm (`yinz-plugin-host/src/lib.rs:458-474`), so in-flight agent work is counted during live-update quiescence. The `AgentControl` path in `yinz_exports` does not count it. |
| D7 | **No Restart, no Ctrl-C, no close in v1.** | `AgentRequest::Restart` has no `expected_session_id` (`yinz-types/src/agent/control.rs:170-174`). `CloseTerminalPane` is unguarded. Ctrl-C can kill the agent. |
| D8 | **The listener is a desired-state loop: it is bound only while `enabled` is set, a valid password file exists, and the session is not quiesced. Stopping never blocks.** | This releases the port during a live update so the replacement can bind it, and takes the web surface down after `set-password --clear` without a restart. `SessionMetaData::drop` never waits. |
| D9 | **Guest entry is a new worker `Operation::Web(Value)` that runs through `yinz-async-lifecycle` with the SDK function `yinz-web-dispatch` as the hook.** It does not use a user-visible `http` callback. | There is no name collision with user code. Deferred results, `%async-check` and `%async-drain` behave exactly as they do for the `palette`/`pipe` lifecycles (`yinz-steel/src/lib.rs:608-649`, `async.scm:358-410`). |
| D10 | **Live-agent presentation is computed in the screen and returned as a plain bus DTO (`WebLiveAgent`). `LiveAgent` is not moved into `yinz-bus`.** | `yinz-plugin-host` does not depend on `yinz-panes`, which owns `AgentPresentation` and `agent_title::status`. The screen extracts small helpers in place (`label_source`, `header_overlay`, `usage_unavailable`, `focus_live_agent`), and the palette keeps using them, so both stay identical. |
| D11 | **HTML nodes are opaque native Steel custom types (`HtmlNode`, `TrustedHtml`).** | If nodes were hash-shaped, agent JSON containing a `"raw"` key could become raw HTML. Opaque types make data and markup impossible to confuse. |

---

## 1. Rust web server

### 1.1 Crate: new `yinz-web/` (library)

- Dependencies: `yinz-types`, `yinz-runtime`, `hyper = { version = "1", default-features = false, features = ["server","http1"] }`, `hyper-util = { version = "0.1", default-features = false, features = ["tokio","server-graceful"] }`, `http = "1"` (1.5.0 is already locked), `http-body-util = "0.1"`, `bytes`, `tokio` (workspace, plus `sync`), `tokio-util` (workspace, for `CancellationToken`), `argon2 = { version = "0.5", default-features = false, features = ["alloc","password-hash"] }`, `getrandom = "0.4"` (already locked at 0.4.3), `sha2` (workspace), `base64 = "0.22"`, `zeroize`, `serde`, `serde_json`, `libc`, `tempfile`.
  - `argon2 0.5.3` uses `blake2 0.10`/`digest 0.10`; WS-A added them to the lock (they were not there before). `hyper-1.12.0` and `hyper-util-0.1.21` are already in the local registry. `hmac` is not needed.
- It does **not** depend on `yinz-bus`. `yinz-server` connects it to the plugin thread through the `Backend` trait, which keeps its unit tests free of threads.
- Modules (`yinz-web/src/`):

| File | Owner | Contents |
|---|---|---|
| `lib.rs` | WS-A (frozen after A) | `mod` declarations and re-exports only |
| `server.rs` | WS-B | `WebServer`, `WebSettings`, `WebStatus`, `Backend` |
| `listener.rs` | WS-B | Desired-state loop, bind, accept, connection cap, drain |
| `service.rs` | WS-B | Request pipeline (§2.6) |
| `request.rs` | WS-B | Path, query and header normalization; body collection |
| `policy.rs` | WS-B | Host, Origin, Funnel, Tailscale-identity checks; security headers |
| `site.rs` | WS-B | Serves `Site` pages and assets, ETag, 304 |
| `pages.rs` | WS-B | Native minimal pages: 404, "starting", "no password" (escaped, no script) |
| `listener_file.rs` | WS-B | `<data_dir>/web/listeners/<pid>-<n>.json` |
| `auth/{mod,password,sessions,limiter,endpoints}.rs` | WS-G | §2 |
| `unit/*.rs` | B and G, split by file | Tests |

### 1.2 Public API (contract between WS-B and the server wiring)

```rust
// yinz-web/src/server.rs
pub type DispatchFuture = Pin<Box<dyn Future<Output = Option<yinz_types::web::WebReply>> + Send>>;
pub trait Backend: Send + Sync + 'static {
    /// None = the reply slot was dropped (plugin thread exited or instance disposed) → 503.
    fn dispatch(&self, request: yinz_types::web::WebRequest) -> DispatchFuture;
    fn site(&self) -> Option<std::sync::Arc<yinz_types::web::Site>>;
    fn quiesced(&self) -> bool;          // senders.session_update_quiesced
    fn activate(&self);                  // sent once per successful bind: load the web plugin
    fn session_name(&self) -> String;    // read per call (sessions can be renamed)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSettings {
    pub bind: std::net::SocketAddr,          // 127.0.0.1:7419, from WebConfig::bind_addr()
    pub allowed_hosts: Vec<String>,          // extra exact hosts, added to loopback + *.ts.net
    pub allowed_origins: Vec<String>,        // extra exact "https://host:port"
    pub tailscale_users: Vec<String>,        // empty = off
    pub session_days: u32,                   // 30
    pub data_dir: std::path::PathBuf,
    pub get_timeout: Duration,               // 15 s
    pub post_timeout: Duration,              // 45 s (peer prompt deadline is 32 s)
    pub header_timeout: Duration,            // 10 s
    pub retry_interval: Duration,            // 3 s (+0–1 s jitter); tests 50 ms
    pub max_connections: usize,              // 64
}
impl WebSettings { pub fn from_config(c: &yinz_types::input::web::WebConfig, data_dir: &Path) -> Result<Self>; }
// Defaults for the fields above, used by from_config:
pub const DEFAULT_GET_TIMEOUT: Duration;     // 15 s
pub const DEFAULT_POST_TIMEOUT: Duration;    // 45 s
pub const DEFAULT_HEADER_TIMEOUT: Duration;  // 10 s
pub const DEFAULT_RETRY_INTERVAL: Duration;  // 3 s
pub const DEFAULT_MAX_CONNECTIONS: usize;    // 64
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebStatus { Disabled, WaitingForPassword, Quiesced, Binding { attempts: u32, last_error: String }, Serving { addr: SocketAddr }, Stopped }
pub struct WebServer { /* CancellationToken, watch::Receiver<WebStatus>, Arc<State> */ }
impl WebServer {
    pub fn start(settings: WebSettings, backend: Arc<dyn Backend>, rt: &tokio::runtime::Handle) -> Self;
    pub fn status(&self) -> WebStatus;
    pub fn local_addr(&self) -> Option<SocketAddr>;
    pub fn stop(&self);                      // cancel only; never blocks
}
impl Drop for WebServer { fn drop(&mut self) { self.stop() } }
```

`yinz-web/src/lib.rs` re-exports `Backend, DispatchFuture, WebServer, WebSettings, WebStatus`. The modules `server`, `listener_file` and `auth` are `pub` (the CLI reads listener files); the others are private. As WS-A left it, `WebServer` holds only the `CancellationToken` and `watch::Receiver<WebStatus>`, and `start` is a stub that never listens and reports `Stopped`; B adds `Arc<State>` and the listener.

All state lives in one `Arc<State>` per `WebServer`: limiter, session cache, request-id counter (an `AtomicU64` seeded randomly) and argon2 semaphore. Nothing is `static`.

### 1.3 Lifecycle

- **Start.** In `yinz-server/src/lib.rs` `init_session` (`:2958`), after the `background_jobs` spawn and before `Ok(())`:
  - if `config.web.enabled()`, build `PluginBackend` (new file `yinz-server/src/web.rs`);
  - call `WebServer::start(settings, backend, get_tokio_runtime().handle())`;
  - store the result in the new field `SessionMetaData.web_server: Option<yinz_web::WebServer>` (`:159`).
  - A bad `address` or `port` value is logged with `.non_fatal()` and the server does not start.
- **Stop.** In `impl Drop for SessionMetaData` (`:319`), immediately after `self.shutdown.begin()`, call `self.web_server.take();`. Dropping cancels; the listener task drops the `TcpListener`, which frees the port at once, then drains connections for 2 s in the background and aborts them. **Drop never blocks.**
- **Desired-state loop (`listener.rs`).** The loop re-evaluates every 1 s while bound, and every `retry_interval` plus jitter otherwise:

  ```
  desired = password_state(data_dir) == Ready && !backend.quiesced()      (enabled is implied by start)
  desired && !bound  → TcpListener::bind
        Ok          → Serving; write listener file; backend.activate(); log info once
        EADDRINUSE  → Binding; log info once per transition ("held by another session")
        other error → Binding; log warn (rate-limited); retry interval ×10
  !desired && bound → drop listener (port freed), drain in background, remove listener file
  ```

  - First come, first served. Whichever session holds the port serves every local session's agents (§4), so which session it is does not matter.
  - Takeover happens within about 3 s after the holder exits.
  - During a live update, the source session becomes quiesced, releases the port within 1 s, and the replacement binds. On rollback, the source rebinds once the port is free.
  - Accept errors never end the loop (vetis bug 2). On `EMFILE`/`ENFILE` it sleeps 100 ms; other errors `continue`.
  - Connections beyond the cap are dropped (`try_acquire_owned` on `Semaphore(64)`).
- **Per-connection settings:** `http1::Builder::new().timer(TokioTimer::new()).header_read_timeout(header_timeout).keep_alive(true).max_buf_size(16 * 1024).half_close(false)`, plus `max_headers(64)` if the builder exposes it.
  - The header-read timer is re-armed on every `poll_read_head` (`hyper-1.12.0/src/proto/h1/conn.rs:224-238`), so it also limits idle keep-alive time. There is also a 5-minute hard connection lifetime, after which the connection gets `graceful_shutdown`.
- **Config reload.** v1 rule: the `web { }` block takes effect at the next session start. `yinz web status` reports the effective values. Live reconfigure is out of scope.
- **Tests.** The block is disabled by default and test configs never set it, so in-process servers never listen unless a test asks. Tests use `port 0` and read `local_addr()` or the listener file.

### 1.4 Config block

The type lives in **`yinz-types/src/input/web.rs`**, next to `hooks.rs`. It is a field on `Config`, not `Options`, so it stays off the client↔server protobuf.

```rust
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebConfig {
    pub enabled: Option<bool>,             // default false
    pub address: Option<String>,           // an IP address, default "127.0.0.1" (not `bind`: see below)
    pub port: Option<u16>,                 // default 7419; 0 = ephemeral
    pub allow_non_loopback: Option<bool>,  // required for a non-loopback address
    pub plugin: Option<String>,            // alias or URL; default alias "web"
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub tailscale_users: Vec<String>,
    pub session_days: Option<u32>,         // 1..=90, default 30
    pub https_port_hint: Option<u16>,      // default 7443; only for printed commands
}
impl WebConfig {
    pub fn merge(self, other: Self) -> Self;          // other's Some/non-empty wins
    pub fn enabled(&self) -> bool;                    // enabled == Some(true)
    pub fn bind_addr(&self) -> Result<SocketAddr, String>; // validates loopback rule
    pub fn plugin_or_default(&self) -> String;        // "web"
    pub fn session_days_or_default(&self) -> u32;     // 30
    pub fn https_port_hint_or_default(&self) -> u16;  // 7443
    pub fn validate(&self) -> Result<(), String>;     // bind_addr() and the 1..=90 day range
}
pub const DEFAULT_WEB_ADDRESS: IpAddr;               // 127.0.0.1
pub const DEFAULT_WEB_PORT: u16;                     // 7419
pub const DEFAULT_WEB_PLUGIN: &str;                  // "web"
pub const DEFAULT_WEB_SESSION_DAYS: u32;             // 30
pub const WEB_SESSION_DAYS_RANGE: RangeInclusive<u32>; // 1..=90
pub const DEFAULT_WEB_HTTPS_PORT_HINT: u16;          // 7443
```

The listen-address setting is **`address`**, not `bind`: the configuration editor keys every node named `bind` as a keybinding, so `["web","bind"]` could never be edited by path. `address` must be an IP address (`"localhost"` is rejected); `[::1]` is accepted only with both brackets. List settings accept several values per line and repeated lines; a repeated scalar setting is a config error ("web port is set twice"). Child blocks and properties are rejected.

```kdl
web {
    enabled true
    // address "127.0.0.1"
    // port 7419
    // plugin "web"                                   // alias or URL of the route plugin
    // allowed_hosts "box.tail1234.ts.net"
    // allowed_origins "https://box.tail1234.ts.net:7443"
    // tailscale_users "me@github"
    // session_days 30
    // https_port_hint 7443
}
```

**When the server is enabled:** `web.enabled == Some(true)` and `bind_addr()` succeeds. A non-loopback address without `allow_non_loopback true`, or `session_days` outside 1..=90, is a config error: `Config::from_kdl` runs `WebConfig::validate()` on the merged config, which also covers edits made through the config editor. A password file is a *serving* condition (§1.3), not a *start* condition.

Files (WS-A):
- `yinz-types/src/input/web.rs` (new) and `yinz-types/src/input/mod.rs`.
- `yinz-config/src/config.rs:33-57`: add `pub web: WebConfig`, and handle it everywhere `ui` is handled in that file.
- `yinz-config/src/kdl/mod.rs`:
  - in `Config::from_kdl` near `:1159`, add `single_top_level_block(&kdl_config, "web")`;
  - in `to_kdl` near `:1217`, push the block only when it differs from the default. This is required, because live update rebuilds the config from the effective text.
- `yinz-config/src/kdl/web.rs` (new): `WebConfig` lives in yinz-types, so yinz-config cannot give it inherent methods. The KDL code is two free functions, `pub(crate) fn from_kdl(&KdlNode) -> Result<WebConfig, ConfigError>` and `pub(crate) fn to_kdl(&WebConfig) -> Option<KdlNode>` (`None` for the default), modelled on `UiConfig::from_kdl` (`kdl/mod.rs:1510`).
- `yinz-config/src/editor.rs`: add `"web"` arms at **both** `:816` (`apply`) and `:897`.
- `yinz-config/assets/config/default.kdl`:
  - a commented `// web { … }` example;
  - in `plugins { }`, the alias `web location="yinz:web" { shared true }`. **Do not add it to `load_plugins`.**
- `example/default.kdl`: the same commented example.
- `yinz-config/src/kdl/unit/duplicate_blocks.rs:5`: add `"web"` to `BLOCKS` and bump the array length. Add `yinz-config/src/kdl/unit/web.rs` (new). Review snapshots.

**Plugin-thread input.** `plugin_thread_main` gains one trailing parameter, `web: Option<yinz_bus::web::WebPluginParams>`. It is `Some` only when web is enabled. The call site is around `yinz-server/src/lib.rs:3264` (WS-A passes `None`).

```rust
pub struct WebPluginParams { pub plugin: RunPluginOrAlias, pub site: yinz_types::web::SiteHandle }
```

The server builds the `RunPluginOrAlias` from `plugin_or_default()`. Use the same string-to-`RunPluginOrAlias` conversion that `hooks { }` values use (`yinz-types/src/input/hooks.rs`). The `SiteHandle` is shared with `PluginBackend`.

---

## 2. Authentication and HTTP hardening

All of this is in Rust, before any request reaches the plugin.

### 2.1 Files on disk (`yinz-web/src/auth/password.rs`, `sessions.rs`)

- Directory `<data_dir>/web/`, created with `yinz_runtime::shared::ensure_secure_dir` (`yinz-runtime/src/shared.rs:30`: 0700, refuses symlinks).
  - **VERIFY:** that it accepts the integration-test temp data dir. If not, fall back to `create_dir_all`, `chmod 0700`, and an `O_NOFOLLOW` check.
- Writes: exclusive `flock` on `web/.lock`, then `NamedTempFile::new_in(web_dir)`, `set_permissions(0o600)`, `write_all`, `sync_all`, `persist`, and fsync of the directory. This copies `host_identity::persist_id` (`yinz-runtime/src/host_identity.rs:77-113`), but the path is passed explicitly and there is no global cache.
- Reads: `O_NOFOLLOW|O_CLOEXEC`, then `fstat`. The file is **Invalid** (serving off, one warning logged) if any of these hold:
  - it is not a regular file, or not owned by the euid;
  - `mode & 0o077 != 0`;
  - it is over 4 KiB, or does not parse;
  - its PHC parameters are outside the bounds: floor `m ≥ 19456`, `t ≥ 2`; ceiling `m ≤ 262144`, `t ≤ 10`, `p ≤ 4`.

**`web/password`**
```json
{"version":1,"phc":"$argon2id$v=19$m=65536,t=3,p=1$<salt>$<hash>","verifier_id":"<32 hex>","updated_at_ms":0}
```
- Argon2id: `Params::new(65536, 3, 1, Some(32))`, `Version::V0x13`, a 16-byte salt from `getrandom::fill`. Tests use a light profile through `#[cfg(test)] fn test_params()`.

**`web/sessions.json`** (0600, ≤ 256 KiB, ≤ 16 sessions; the one with the oldest `last_seen` is evicted)
```json
{"version":1,"verifier_id":"<must equal password.verifier_id>",
 "sessions":[{"id":"<16 hex public id>","token_sha256":"<64 hex>","label":"iPhone Safari",
              "created_at_ms":0,"last_seen_ms":0,"expires_at_ms":0,"remember":true}]}
```
- The file is shared by every server on the data dir, so logout and revocation work across port takeovers and live updates.
- Each server caches the parsed file, keyed by `(dev, ino, mtime_ns, len)`, and calls `stat` once per authenticated request.
- `last_seen` updates stay in memory and are flushed at most every 5 minutes, and on logout.

**API (WS-G; used by WS-B and the CLI):**
```rust
pub mod password {
    pub fn write(data_dir: &Path, password: &zeroize::Zeroizing<String>) -> Result<()>; // new verifier_id, empties sessions
    pub fn revoke_sessions(data_dir: &Path) -> Result<()>;                              // new verifier_id, empties sessions
    pub fn clear(data_dir: &Path) -> Result<bool>;                                      // removes both files
    pub fn read_state(data_dir: &Path) -> PasswordState;                                // Missing | Invalid(String) | Ready(Verifier)
    pub fn verify(v: &Verifier, candidate: &[u8]) -> bool;                              // blocking
}
pub struct Auth;  // in auth/mod.rs
impl Auth {
    pub fn new(data_dir: PathBuf, cfg: AuthConfig, clock: Arc<dyn Clock>) -> Self;
    pub fn password_state(&self) -> PasswordStateKind;                  // Missing | Invalid | Ready (stat-cached, ≤1 s)
    pub fn authenticate(&self, bearer: Option<&str>) -> Result<SessionInfo, AuthError>;
    pub async fn login(&self, body: zeroize::Zeroizing<Vec<u8>>, user_agent: Option<&str>, client_key: &str) -> AuthResponse;
    pub fn logout(&self, s: &SessionInfo) -> AuthResponse;
    pub fn logout_all(&self) -> AuthResponse;
    pub fn session_json(&self, s: &SessionInfo) -> AuthResponse;
    pub fn touch(&self, s: &SessionInfo) -> Result<(), RateLimited>;  // per-session API bucket + last_seen
}
pub struct SessionInfo { pub id: String, pub label: String, pub expires_at_ms: i64 }
pub struct AuthResponse { pub status: u16, pub json: serde_json::Value, pub retry_after_s: Option<u64> }
// Types named above, as WS-A defined them:
pub trait Clock: Send + Sync { fn now_ms(&self) -> i64; }  pub struct SystemClock;
pub struct AuthConfig { pub session_days: u32 }
pub enum PasswordStateKind { Missing, Invalid, Ready }
pub enum AuthError { Unauthenticated, PasswordNotReady }
pub struct RateLimited { pub retry_after_s: u64 }
// in password: Verifier { phc, verifier_id, updated_at_ms } (Debug hides phc);
//              PasswordState { Missing, Invalid(String), Ready(Verifier) }
```

WS-A's stub bodies deny everything: `read_state` returns `Missing`, `verify` returns false, `write`/`revoke_sessions`/`clear` return errors, login answers 503 `no_password`, and logout, logout-all and session answer 401 `unauthenticated`.

On day one, WS-B creates `auth/mod.rs` with these exact signatures and deny-all bodies. WS-G replaces the bodies.

### 2.2 Tokens

- Format: `yzw1_` followed by base64url (no padding) of 32 random bytes.
- The server looks sessions up by `sha256(token)`. Raw tokens are never stored or compared.
- A session is valid only if all of these hold:
  - the hash matches;
  - `now < expires_at_ms`, where `expires_at_ms = created + session_days`;
  - `now - last_seen < idle`, where idle is 14 days with `remember` and 12 hours without;
  - `sessions.verifier_id == password.verifier_id`;
  - the password state is Ready.
- The frontend stores the token in `localStorage` when `remember` is set, otherwise in `sessionStorage`.
- The plugin never sees the token. It receives `auth = {session: <public id>, label}`.

### 2.3 Native endpoints (never routed to the plugin)

| Method and path | Auth | Behaviour |
|---|---|---|
| `POST /api/login` | none | JSON `{password, label?, remember?}`, ≤ 4 KiB. Success: `200 {"token","session":{id,label,expires_at_ms}}`. Every failure: `401 {"error":{"code":"invalid_credentials"}}`. Rate-limited: `429` with `Retry-After`. Password not Ready: `503 {"error":{"code":"no_password"}}`. |
| `POST /api/logout` | bearer | `204`; deletes this session |
| `POST /api/logout-all` | bearer | `204`; same as `revoke_sessions` |
| `GET /api/session` | bearer | `200 {id,label,created_at_ms,expires_at_ms,idle_expires_at_ms}` |
| `GET /healthz` | none | `200 {"ok":true}`. Adds `"session": name` only when the peer is loopback and there is no `X-Forwarded-For` (a direct probe from `yinz web status`). |

- Login runs argon2 in `spawn_blocking` under `Semaphore(1)`, with at most 2 waiters; more get 429. This caps memory at about 64 MiB.
- The body is read into `Zeroizing<Vec<u8>>`.

### 2.4 Rate limiting (per `WebServer`)

- **Login key:** `Tailscale-User-Login` if present and the peer is loopback; else the first `X-Forwarded-For` hop if the peer is loopback; else the peer IP.
  - 5 failures in 15 minutes locks that key out for 60 s, doubling up to 15 minutes.
  - Success resets the key.
- **Global login ceiling:** 30 failures per hour, then a 15-minute lockout for new logins. Existing sessions keep working.
- **Authenticated API:** a token bucket per session, 20 requests per second with a burst of 40; beyond that, 429.
- **Public site** (`/`, `/login`, `/static/*`): a global bucket, 50 requests per second with a burst of 100.
- **Plugin in-flight cap:** 64 per instance, enforced by `WebPending` (§3.3); beyond that, 503 with `Retry-After: 2`.
- Minimum password length is 12 characters, enforced by the CLI.

### 2.5 Public paths

- `GET|HEAD /`, `/login`, `/static/**`: served from the frozen `Site`.
- `POST /api/login`, `GET /healthz`.

Everything else under `/api/` needs a bearer token; without one it gets `401 {"error":{"code":"unauthenticated"}}` with `WWW-Authenticate: Bearer`. A non-API GET that is not in the `Site` gets the native 404 page. A non-API POST gets 405.

### 2.6 Pipeline (`service.rs`)

Every outcome is an `Ok(Response)`; the service never returns `Err`. Steps in order:

1. **Method.** Only GET, HEAD and POST; otherwise 405. OPTIONS gets 405, and no CORS headers are ever sent.
2. **Funnel guard.** A request with a `Tailscale-Funnel-Request` header gets 403, and a loud warning is logged (rate-limited). **VERIFY** the header name against the Tailscale docs.
3. **Host check.** Take the hostname from `Host`, lowercase it, ignore the port. It must be one of:
   - `localhost`, `127.0.0.1` or `::1`;
   - a name ending in `.ts.net`;
   - an entry in `allowed_hosts`.
   Otherwise 421.
4. **Tailscale identity.** If `tailscale_users` is set, `Tailscale-User-Login`, trusted only from a loopback peer, must be in the list; otherwise 403. This is in addition to the password.
5. **Path normalization.**
   - The path must start with `/`, be ≤ 2048 bytes, and have ≤ 32 segments.
   - Reject `\0`, `\`, `//`, and `%2f`, `%5c` or `%00` anywhere.
   - Percent-decode each segment once. Each must be UTF-8 and not `.` or `..`.
   - The query must be ≤ 2048 bytes and decode to ≤ 32 pairs (`form_urlencoded`).
   - Violations get 400.
6. **Route class.**
   - `GET /healthz` and the native auth endpoints → §2.3.
   - `/api/*` → steps 7–11.
   - Any other GET/HEAD → `Site` lookup:
     - hit → 200, or 304 on `If-None-Match`;
     - `backend.site()` is `None` → 503 "Yinz web is starting" page with `Retry-After: 1`;
     - miss → 404 page.
7. **Bearer authentication** (`auth.authenticate`), then `auth.touch` (429).
8. **POST checks.**
   - If `Sec-Fetch-Site` is `cross-site`, 403.
   - `Origin` is required. Its exact `scheme://host:port` must equal one of: the request `Host` with scheme `https` (or `http` for loopback hosts); `https://` + `X-Forwarded-Host` (loopback peer only); or an entry in `allowed_origins`. Otherwise 403.
   - `Content-Type` must be `application/json`, otherwise 415.
   - These checks also apply to `POST /api/login`.
9. **Quiesce.** If `backend.quiesced()`, a POST gets 503 `{"error":{"code":"updating"}}` with `Retry-After: 5`. This is the same rule as `yinz-server/src/client_router.rs:512`.
10. **Body.**
    - If `Content-Length` is over 64 KiB, 413 before reading. Otherwise read through `Limited` under a 10 s timeout (408).
    - The body must be UTF-8 (400).
    - A GET with a body gets 400.
11. **Forward.** Build `WebRequest`, then `timeout(get_timeout | post_timeout, backend.dispatch(req))`:

| Result | Status |
|---|---|
| Timeout | 504 `{"error":{"code":"timeout"}}` |
| `None` | 503 |
| `Unavailable{retry_after_s}` | 503 with `Retry-After` |
| `Failed` | 502 |
| `Response` | validated in §3.4, then sent |

**Security headers** are set natively on every response, and plugins cannot set them:
```
Content-Security-Policy: default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:;
  font-src 'self'; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'none';
  frame-ancestors 'none'
X-Content-Type-Options: nosniff
Referrer-Policy: no-referrer
X-Frame-Options: DENY
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Resource-Policy: same-origin
Permissions-Policy: camera=(), microphone=(), geolocation=()
Cache-Control: no-store                 (API, native pages; Vary: Authorization on /api/*)
Cache-Control: no-cache + strong ETag   (Site pages and assets)
```

- `<script type="module" src="/static/app.js">` and relative imports satisfy `script-src 'self'`. Inline import maps are blocked, so the vendored bundle must have no bare imports (§6).
- `<script type="application/json">` is never executed, so it is CSP-safe.
- Injected HTML can neither run inline handlers nor load cross-origin images. No HSTS: it applies to the whole host and would affect the other services on it.

**Logging:**
- Debug-level access lines record method, route class (`api`, `site`, `auth`), status, milliseconds, and an 8-hex session prefix.
- Never logged: passwords, tokens, hashes, `Authorization` values, bodies, query strings.
- Auth events (login ok, login failed with count, lockout, revoke) are logged at info, rate-limited, because `yinz.log` is truncated at 100 KB.

---

## 3. Native → Steel request/reply seam

### 3.1 Seam types

**`yinz-types/src/web.rs`** (new, WS-A; std and serde only):
```rust
pub const MAX_REQUEST_BODY: usize = 64 * 1024;
pub const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)] #[serde(rename_all = "UPPERCASE")]
pub enum WebMethod { Get, Head, Post }
#[derive(Clone, Debug, Serialize)] pub struct WebAuth { pub session: String, pub label: String }
#[derive(Clone, Serialize)]
pub struct WebRequest {
    pub id: u64, pub method: WebMethod, pub path: String, pub segments: Vec<String>,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,      // lowercase allowlist: accept, accept-language, content-type, if-none-match, user-agent
    pub body: Option<String>, pub auth: WebAuth, pub deadline_ms: i64,
}
impl std::fmt::Debug for WebRequest { /* id, method, path, body length only */ }
#[derive(Debug)] pub enum WebReply { Response(WebResponse), Unavailable { reason: String, retry_after_s: u32 }, Failed(String) }
#[derive(Debug)] pub struct WebResponse { pub status: u16, pub headers: Vec<(String, String)>, pub body: String }
#[derive(Debug)] pub struct StaticFile { pub bytes: std::sync::Arc<[u8]>, pub content_type: &'static str, pub etag: String }
#[derive(Debug, Default)]
pub struct Site {
    pub plugin_id: Option<crate::internal_ids::PluginId>, pub generation: u64,
    pub assets: std::collections::BTreeMap<String /*url prefix "/static/"*/, std::collections::BTreeMap<String /*relative*/, StaticFile>>,
    pub pages: std::collections::BTreeMap<String /*exact path*/, StaticFile>,
}
impl Site { pub fn lookup(&self, path: &str) -> Option<&StaticFile>; }
#[derive(Clone, Default)] pub struct SiteHandle(std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<Site>>>>);
impl SiteHandle { pub fn current(&self) -> Option<Arc<Site>>; pub fn set(&self, site: Option<Arc<Site>>); }
```

**`yinz-bus/src/web.rs`** (new, WS-A):
```rust
/// Shared so the first send wins on any clone; PluginInstruction is Clone.
#[derive(Clone)] pub struct ReplySlot(Arc<Mutex<Option<tokio::sync::oneshot::Sender<WebReply>>>>);
impl ReplySlot {
    pub fn new() -> (Self, tokio::sync::oneshot::Receiver<WebReply>);
    pub fn send(&self, reply: WebReply) -> bool;
    pub fn is_closed(&self) -> bool;
}
impl Debug for ReplySlot { "ReplySlot" }
#[derive(Clone, Debug)] pub enum WebInstruction {
    Request { request: Box<WebRequest>, reply: ReplySlot },
    Activate,
    AgentView(Box<AgentViewCall>),
}
#[derive(Clone)] pub struct AgentViewCall { pub plugin_id: PluginId, pub client: ClientId, pub generation: Option<u64>, pub id: String, pub request: String }
impl Debug for AgentViewCall { /* every field but `request`, which can carry prompt text; prints request_len */ }
pub struct WebPluginParams { pub plugin: RunPluginOrAlias, pub site: SiteHandle }   // Clone, Debug
// Screen DTOs (§4.1)
pub struct WebLiveAgent { … }  pub struct WebLiveHeader { … }  pub struct WebAgentRef { … }
```

**Bus variants (WS-A):**
- `PluginInstruction::Web(web::WebInstruction)` in `yinz-bus/src/plugins.rs`; add `PluginContext::Web` (it resolves by name, so `From<&PluginInstruction>` needs no hand-written arm); add the name in `yinz-types/src/diagnostics.rs`.
- `ScreenInstruction::WebLiveAgents { reply: crossbeam::channel::Sender<Vec<WebLiveAgent>> }` and `ScreenInstruction::WebFocusAgent { agent: WebAgentRef, reply: crossbeam::channel::Sender<Result<(), String>> }` in `yinz-bus/src/screen.rs`, with `ScreenContext` arms.

**`PluginBackend`** (`yinz-server/src/web.rs`, WS-B):
- `dispatch`: create a `ReplySlot`, send `PluginInstruction::Web(Request{..})`, and await the receiver. `to_plugin` is unbounded, so sending from tokio is safe.
- `activate`: send `Web(Activate)`.
- `site`: `site_handle.current()`.
- `quiesced`: `senders.session_update_quiesced.load(Acquire)`.
- `session_name`: read from `senders.session`.

### 3.2 Plugin thread (`yinz-plugin-host/src/web_requests.rs`, new, WS-C)

```rust
pub(crate) struct WebRequests { params: Option<WebPluginParams>, host: WebHost, target: Option<PluginId>, last_start: Option<Instant> }
impl WebRequests {
    pub fn new(params: Option<WebPluginParams>, host: &WebHost) -> Self;
    pub fn activate(&mut self, bridge: &mut PluginBridge, client: ClientId);
    pub fn request(&mut self, bridge: &mut PluginBridge, req: Box<WebRequest>, reply: ReplySlot, client: ClientId,
                   shutdown: tokio::sync::mpsc::Sender<()>);
}
```

WS-A built `activate` in full; `request` is a stub that answers `Unavailable{"web plugin unavailable", 5}` when `params` is `None`, `Unavailable{"starting", 1}` (after `activate`) when there is no target, and otherwise `Unavailable{"web plugin unavailable", 5}`. C replaces the last case with steps 3 and 4 below.
```

The plugin-thread loop (`lib.rs`) owns it as a local value, like `update_pause`. It is never global.

**`Activate`:** the `lib.rs` arm skips it while `update_pause.is_some()`; the next `Request` after resume activates through step 2.
1. If `params` is `None`, do nothing.
2. Otherwise call `bridge.start_background_plugin_then(params.plugin, bridge.get_first_client_id().unwrap_or(initiating_client_id), true, |id| host.set_target(Some(id)))`.
   - `start_background_plugin_then` calls the closure with the plugin's id **before** `load_plugin` queues guest initialization on the executor (and with the existing id when an instance already runs). The target is therefore set before the guest's `load` runs, so a web plugin may call `HttpAssets`/`HttpPublish` from `load`, and C's `is_target` gate sees it.
3. On `Ok`, record `target = Some(id)`. On `Err`, clear both `target` and the `WebHost` target.

The web plugin therefore loads only in the session that actually holds the port.

**`Request`:**
1. If `update_pause.is_some()`, reply `Unavailable{"session update in progress", 5}`. This check is in the `lib.rs` arm, not in `WebRequests`. If `params` is `None`, reply `Unavailable{"web plugin unavailable", 5}`.
2. If there is no target, call `activate`, then reply `Unavailable{"starting", 1}`.
3. If the target instance is missing, or every instance `is_unavailable()`:
   - reply `Unavailable{"web plugin unavailable", 5}`;
   - if `last_start` is more than 30 s ago, **restart** it: `bridge.unload_plugin(id)` followed by `start_background_plugin(.., true)`. `start_background_plugin` alone would return the existing failed instance (`background_plugins.rs:120-127`).
4. Otherwise call `bridge.plugin_web(id, req, reply, shutdown)`.

**`PluginBridge::plugin_web`** (`plugin_bridge.rs`, modelled on `plugin_palette` at `:1405-1485`):
- It uses `execute_for_plugin_tagged(id, "plugin_web", false, |senders, plugin_map, connected_clients| …)`.
- Find the first running instance of `id` that is not `is_unavailable()`. If there is none, `reply.send(Unavailable)`.
- Insert into the env's pending map: `plugin.env().web_pending.lock().insert(req.id, reply.clone())`.
  - The insert sweeps closed slots first.
  - If the map holds 64 entries, `reply.send(Unavailable{"busy",2})` and return.
- **Acting client.** Use the first connected client if there is one, otherwise leave `env.client_id` unchanged:
  `let acting = connected_clients.lock().unwrap().first().copied(); let previous = acting.map(|c| replace(&mut plugin.env_mut().client_id, c));`
  - When no client is connected, `env.client_id` names a client that is absent, so client-scoped commands are dropped by the screen. The request carries `"attached": false`, focus answers 409, and web handlers must issue no client-scoped commands.
  - Do **not** copy palette's "client must be connected" refusal. Detached use from a phone is the main case.
  - Never use the owner when a real client is connected; that follows `steel/CLAUDE.md`.
- Build the guest JSON: `serde_json::to_value(&*req)` plus `"client"` (acting id or null), `"attached"` and `"server": {"session": env.server_session name}`.
- Call `plugin.plugin_web(&json)` (new wrapper in `plugin_map.rs` next to `plugin_palette` at `:1181`, which calls `SteelPlugin::web`), then restore the client.
- On `Err`:
  - remove the slot and send `Failed("plugin error")`;
  - call `plugin.mark_failed()` and `handle_plugin_crash(..)` exactly as at `plugin_bridge.rs:1470-1480`.
- Do not hold the plugin-map lock across the call longer than palette does.

**`AgentView`:** see D6 and §4.2. The arm in `lib.rs`:
1. If `update_pause` is set, deliver `{"id":…, "reply":{"error":{"code":"paused",…}}}` — the same shape as a normal reply, so E reads `reply.error` in both cases.
2. Otherwise:
   ```rust
   let t = native_tasks.start();
   spawn_blocking(move || {
       let _t = t;
       let payload = yinz_exports::web_agents::handle(&call.request, call.client, &senders, &data);
       senders.send_to_plugin(PluginInstruction::for_plugin_event(call.plugin_id, call.client, call.generation,
           Event::CustomMessage("agent-view-response".into(), json!({"id": call.id, "reply": payload}).to_string())))
   });
   ```

### 3.3 Shared host state (`yinz-plugin-host/src/web_host.rs`, new, WS-C)

```rust
#[derive(Clone, Default)] pub(crate) struct WebHost(Arc<Mutex<State>>);   // per session; created in plugin_thread_main
struct State { target: Option<PluginId>, site: Option<SiteHandle> }
impl WebHost {
    pub fn new(site: Option<SiteHandle>) -> Self;
    pub fn set_target(&self, id: Option<PluginId>);
    pub fn is_target(&self, id: PluginId) -> bool;
    pub fn publish_assets(&self, id: PluginId, generation: u64, prefix: String, files: BTreeMap<String, StaticFile>) -> Result<()>;
    pub fn publish_page(&self, id: PluginId, generation: u64, path: String, file: StaticFile) -> Result<()>;
    pub fn retire(&self, generation: u64);  // clears site if site.generation == generation
}
pub(crate) type WebPending = HashMap<u64, ReplySlot>;
```

- `PluginEnv` (`plugin_map.rs:799`) gains `pub(crate) web: WebHost` (cloned from the bridge, like `panes`) and `pub(crate) web_pending: Arc<Mutex<WebPending>>` (fresh per instance).
  - **VERIFY** every place a `PluginEnv` is built, including named workers in `background_worker.rs`. A worker gets an empty `web_pending` and cannot answer requests.
- Publishing only happens when `is_target(id)` and the session has a `SiteHandle`. If `site.generation != generation` (or another plugin id published it), it starts a fresh `Site`.
- **As WS-A left it:** `web_host.rs` exists with this API and a working body. `plugin_thread_main` creates the `WebHost` and hands it to `WebRequests`, but does not yet pass it to `PluginBridge`/`PluginEnv`; that is C's. `WebPending`, `is_target`, `publish_assets`, `publish_page` and `retire` carry `#[expect(dead_code, reason = …)]`. **C must delete each `#[expect]` as it starts using the item, or `cargo x lint` fails** (an unfulfilled expectation is an error).
- Dispose: in `SteelPlugin::dispose_with_cleanup` (`steel/mod.rs:440`), next to `clear_agent_providers` (`:509`), clear `web_pending` (slots drop, so waiting requests get 503 at once) and call `web.retire(generation)`.

### 3.4 Steel-only host commands (WS-A types; WS-C handlers)

Add these to `PluginCommand` in `yinz-types/src/data.rs` (`CommandType` is derived):
```rust
/// Steel-only. Answers one web request delivered to this instance.
HttpRespond { request_id: u64, response: String },
/// Steel-only. Freezes `directory` (plugin-relative, or "builtin:web/static") under URL `prefix`.
HttpAssets { prefix: String, directory: String },
/// Steel-only. Publishes a public, pre-rendered page (≤ 512 KiB).
HttpPublish { path: String, content_type: String, body: String },
/// Steel-only. Native agent-view operation; replies with CustomMessage "agent-view-response".
AgentView { id: String, request: String },
```

Required edits for each command:
- `plugin_command.rs`: an `Err("Web commands are Steel-only")` arm next to `:3730`.
- `unit/plugin_command_tests.rs`: `=> return None` next to `:629`.
- `test_generators.rs`: a generator arm per `CommandType`.
- `yinz-types/src/diagnostics.rs` and `docs/PLUGIN_PROTOCOL.md`.
- `yinz_exports/permissions.rs`: all four map to `PermissionType::RunCommands`, next to `AgentControl` at `:283`.
- Identity gates: `HttpAssets`, `HttpPublish` and `AgentView` additionally require `env.plugin_config.is_builtin() || env.web.is_target(env.plugin_id)`; otherwise the guest gets the error "reserved for the configured web plugin". `HttpRespond` is gated by `web_pending` membership.
  - **Where:** the command arms in `yinz_exports.rs` call `web::is_web_plugin(env)` in `yinz_exports/web.rs`, which also holds the error text as `web::RESERVED`. WS-A's body returns only `env.plugin_config.is_builtin()`; **C adds `|| env.web.is_target(env.plugin_id)` there** once `PluginEnv.web` exists. The `AgentView` arm (final) already calls it.
  - Handler signatures in `yinz_exports/web.rs`: `respond(&mut PluginEnv, request_id: u64, response: String) -> Result<()>`, `assets(&mut PluginEnv, prefix: String, directory: String) -> Result<()>`, `publish(&mut PluginEnv, path: String, content_type: String, body: String) -> Result<()>`.
  - Protobuf: `plugin_command.rs` refuses all four with "Web commands are Steel-only".

**`HttpRespond` handler** (`yinz-plugin-host/src/yinz_exports/web.rs`, new, WS-C). The `response` string is JSON:
```json
{"status":200,"headers":{"content-type":"application/json; charset=utf-8","etag":"\"…\"","cache-control":"no-store"},"body":"…"}
```
- `status` must be 200–599.
- Header names come from an allowlist: `content-type`, `etag`, `cache-control`, `vary`, `retry-after`, `location` (3xx only, must start with `/` and not `//`), `x-yinz-*`. **Any other header fails closed:** the reply becomes `Failed("disallowed header <name>")`, which gives 502, and is logged once.
- `body` ≤ 4 MiB. A 304 must have an empty body. `content-type` is required when the body is non-empty.
- Take the slot from `env.web_pending` and send `Response`. An unknown id gets a debug log and `Ok(())`; it is either late or a duplicate.

**`HttpAssets` handler:**
1. Resolve the root.
   - Plugin-relative: `source_path(env.resolved_source_path.parent()).join(directory)`.
   - `builtin:<rel>`: `builtin_generation::builtin_path(&env.yinz_data_dir, "web")?.parent().join(rel)`, which lets a user's replacement plugin reuse the shipped frontend.
   - `prefix` must match `^/[a-z0-9/_-]*/$` and must not start with `/api/`. At most 4 prefixes.
2. `spawn_blocking` → `web_assets::load_tree(root)`:
   - take `source_read_guard`;
   - open the root with `O_DIRECTORY|O_NOFOLLOW`;
   - list entries with `fs::read_dir` and open each file with `yinz_runtime::fs_beneath::open_beneath` (the `freeze_sources` pattern, `modules.rs:46-80`);
   - regular files only, depth ≤ 6, ≤ 256 files, ≤ 2 MiB each, ≤ 8 MiB in total;
   - extension allowlist: `js`/`mjs` → `text/javascript; charset=utf-8`, `css` → `text/css; charset=utf-8`, `svg` → `image/svg+xml`, `png` → `image/png`, `ico` → `image/x-icon`, `webmanifest` → `application/manifest+json`, `json` → `application/json`, `woff2` → `font/woff2`, `txt` → `text/plain; charset=utf-8`. Other extensions are skipped.
   - ETag: `"` + first 16 bytes of `sha256(bytes)` in hex + `"`.
3. `env.web.publish_assets(id, generation, prefix, files)`.

**`HttpPublish` handler:**
- `path` must be exact (`/`, `/login`, or `^/[a-z0-9/_.-]+$`), not under `/api/` or any asset prefix.
- `content_type` must be one of `text/html; charset=utf-8`, `application/manifest+json` or `text/plain; charset=utf-8`.
- At most 16 pages.
- Calls `publish_page`.

### 3.5 Executor → guest (WS-E for yinz-steel, WS-C for the host wrapper)

- `yinz-steel/src/worker/protocol.rs:22`: add `Operation::Web(Value)`.
- `worker/server.rs`: add an arm `Operation::Web(m) => Output::Changed(runtime.web(m)?)`, as `Palette` does.
- `worker/client.rs`: add `pub fn web(&mut self, v: Value) -> Result<bool> { self.changed(Operation::Web(v)) }`, next to `palette` at `:180`.
- `yinz-steel/src/lib.rs`:
  ```rust
  pub fn web(&mut self, request: Value) -> Result<bool> {
      let hook = self.optional_function("yinz-web-dispatch")?.unwrap_or(SteelVal::BoolV(false));
      Ok(matches!(self.call_optional("yinz-async-lifecycle",
          vec![SteelVal::StringV("web".into()), hook, SteelVal::ListV(vec![to_scheme(&request)?].into()), SteelVal::BoolV(false)])?,
          Some(SteelVal::BoolV(true))))
  }
  ```
  The `else` branch of `yinz-async-lifecycle` (`async.scm:395-410`) tracks the hook result and polls. `yinz-web-dispatch` returns `#f` synchronously and answers through `HttpRespond`.
- `yinz-plugin-host/src/steel/mod.rs`, next to `palette` at `:416`:
  ```rust
  pub fn web(&mut self, request: &serde_json::Value) -> Result<bool> { self.with_runtime(true, |rt| rt.web(request.clone())) }
  ```
- **Concurrency.** One instance runs one call at a time on its pinned executor. Handlers return deferreds, such as waiting on `AgentView`, so each executor call finishes quickly and requests interleave. The 30 s worker `DEADLINE` bounds any single synchronous call.

### 3.6 `retained_source` and `fs_beneath` (WS-C)

- `yinz-runtime/src/builtin_generation.rs:237-243`: change the function to `retained_source(root: &Path, path: &Path) -> bool`. It keeps Scheme files anywhere, plus files with the allowlisted asset extensions above when a component of `path.strip_prefix(root)` equals `static`. Update the callers in `fingerprint` (`:221`) and `copy_sources` (`:276`). Existing limits still apply (4096 entries, 8 MiB per file, 64 MiB total).
- `yinz-runtime/src/fs_beneath.rs` (new): move `open_beneath` here verbatim from `yinz-steel/src/modules.rs:524-547` as `pub fn open_beneath(root: &File, path: &Path) -> Result<File>`, plus `pub fn open_root(path: &Path) -> Result<File>` (`O_DIRECTORY|O_NOFOLLOW|O_NONBLOCK`). `modules.rs` switches to the moved function; this one-file edit belongs to WS-C.

### 3.7 Guest request hash (what `yinz-web-dispatch` receives)

```scheme
(hash "id" 17 "method" "GET" "path" "/api/agents" "segments" '("api" "agents")
      "query" (hash "run" "claude:abc")          ; last value wins
      "query_pairs" (list (list "run" "claude:abc"))
      "headers" (hash "accept" "application/json")
      "body" json-null                            ; or UTF-8 string
      "auth" (hash "session" "1a2b3c4d5e6f7a8b" "label" "iPhone")
      "client" 3 "attached" #t                    ; client json-null when detached
      "server" (hash "session" "main")
      "deadline_ms" 1762500015000)
```

---

## 4. Agent data and actions: parity through native reuse

### 4.1 Screen side (WS-D)

**DTOs** (in `yinz-bus/src/web.rs`, owned by WS-A):
```rust
#[derive(Clone, Debug)]
pub struct WebLiveAgent {
    pub id: String,                       // LiveAgent::id()
    pub session: SessionName, pub current_session: bool,
    pub pane: PaneInfo, pub metadata: BTreeMap<String, String>,
    pub run: Option<String>,              // LiveAgent::run()
    pub label_source: AgentLabelSource,   // exactly what the palette sends (command_palette.rs:1023-1042)
    pub provider: String, pub activity: String, pub host: Option<String>, pub pane_label: String, // AgentPresentation::from_pane
    pub glyph: &'static str, pub status: &'static str,  // yinz_panes::ui::agent_title::status
    pub focused: bool,                    // current session && pane == active pane of any connected client
    pub selectable: bool, pub suppressed: bool,
    pub usage_unavailable: Option<&'static str>,
    pub details: String,                  // LiveAgent::text()
    pub header: WebLiveHeader,
}
#[derive(Clone, Debug, Default)]
pub struct WebLiveHeader { pub name: String, pub status: String, pub location: String, pub task: String, pub project: String, pub session_id: Option<String> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebAgentRef { pub id: String, pub kind: String, pub session_id: String }
impl WebLiveAgent { pub fn matches(&self, r: &WebAgentRef) -> bool; } // id == r.id, pane.agent.kind == r.kind, pane.agent.session_id == r.session_id; all three non-empty (no separate kind/session_id fields)
```

**Refactors in place in `yinz-screen/src/screen/palette_history.rs`.** Each helper is extracted from existing code and the palette is switched to call it, so the palette and the web view stay identical:
- `LiveAgent::label_source(&self) -> AgentLabelSource`, from `command_palette.rs:1023-1042`. The palette's `refresh_labels` call uses it.
- `LiveAgent::header_overlay(&self, labels: Option<&AgentLabels>) -> WebLiveHeader`, from `palette_history_reply` `:569-610`: name, status `[7] · claude · needs input`, location, task, project fallback, session id. `palette_history_reply` uses it, then applies `details = live.text() + "\n" + details` and usage as before.
- `LiveAgent::usage_unavailable(&self, current: &SessionName) -> Option<&'static str>`, from `refresh_palette_usage` `:749-756`.
- `Screen::focus_live_agent(&mut self, client, target: &LiveAgent) -> Result<(), String>`, from `focus_palette_agent` `:965-997`:
  - keeps the checks `same`, `is_selectable` and `!is_suppressed`;
  - keeps the action choice: `FocusPaneByPaneId` in the same session, `SwitchSession{pane_id}` for a peer;
  - the pane and tab come from the caller. The palette passes its context; the web path passes the client's active tab and pane via `get_active_tab(client)` and `get_active_pane_id`.
  - `focus_palette_agent` keeps the palette-closing step and the error toast.
- Make `same` and `text` `pub(in crate::screen)`.

**`yinz-screen/src/screen/web_agents.rs`** (new), dispatched from `thread_main.rs`:
- `WebLiveAgents`: map `live_palette_agents()` (`:210`) into `WebLiveAgent`, then send on `reply`.
- `WebFocusAgent`:
  1. Use `get_first_client_id()`; with none, reply `Err("No terminal attached")`.
  2. Find the live agent with `matches_ref`; with none, reply `Err("Agent disappeared or changed identity")`.
  3. Call `focus_live_agent`.

### 4.2 Plugin-host side: `yinz-plugin-host/src/web_agents.rs` (WS-D)

Declare it in `yinz_exports.rs` as `#[path = "web_agents.rs"] pub(crate) mod web_agents;` (WS-A writes the stub), next to `palette_agents` (`yinz_exports.rs:43-44`).

Visibility changes in `yinz-plugin-host/src/palette_agents.rs` (WS-D):
- `mod history` → `pub(super) mod history` (it is `#[path = "palette_history.rs"] mod history;` at `:23-24`).
- `execute` → `pub(super)`.
- Split `labels()` into `pub(super) fn compute_labels(sources: Vec<AgentLabelSource>, data: &DataAccess) -> Result<BTreeMap<String, AgentLabels>, String>`, which contains the `resolve_labels` and `last_reply` loop. `labels()` becomes a wrapper that sends to the screen.
- `prompt_peer_with_deadline` → `pub(super) fn control_peer(session, request: AgentRequest, deadline: Instant) -> Result<AgentResponse, String>` (the body is already generic; `prompt_peer` calls it). The `UNCERTAIN` message stays.

In `palette_history.rs`: `prompt_library` stays `pub(super)`. That is enough, because `web_agents` is a child of `yinz_exports` and `history` becomes `pub(super)` inside `palette_agents`.

```rust
pub(crate) fn handle(request: &str, client: ClientId, senders: &ThreadSenders, data: &DataAccess) -> serde_json::Value
// returns {"ok": …} | {"error": {"code": "invalid|not_found|target_changed|paused|unavailable|failed", "message": "…"}}
```

**Request** (`#[serde(tag = "op", rename_all = "snake_case")]`; `AgentRef = WebAgentRef`):

| op | Fields | Native path reused |
|---|---|---|
| `list` | – | `WebLiveAgents` (5 s `recv_timeout`) → `compute_labels(label_sources)` → sort by `LiveAgentOrder::new(state, last_hook_at_ms, project, branch, pane.id, session)` (`yinz-types/src/agent.rs:67`) |
| `history` | `agent?: String` (id), `run`, `from?`, `query` (default `""`), `tz?` | Build the palette `AgentRequest` (below) → `execute(AgentAction::History(HistoryAction::Run{id: run, from, query}))`. If `agent` is in the snapshot, overlay `header` with `WebLiveHeader` and prefix `details`. If the agent's `run` changed, set `retargeted: true` and the new `run`, as the palette's history tick does. If the agent is missing, set `agent_gone: true` and still serve the run. |
| `hit` | `run`, `revision`, `tz?` | `execute(History(Hit))` |
| `event` | `run`, `revision`, `tz?` | `execute(History(Event))` → `Reader` |
| `tools` | `run`, `revisions[]` (≤ 30), `tz?` | `execute(History(Tools))` → `Reader` |
| `usage` | `agent` (id) | `usage_unavailable` from the snapshot; otherwise `crate::provider_usage::collect(kind.replace('◆',""))` |
| `prompts` | – | `history::prompt_library(data)`, capped at 500 |
| `prompt` | `agent: AgentRef`, `submission_id` (≤ 64 chars), `text` (≤ 32 KiB) | Fresh snapshot, `matches`, else `target_changed`. Then `execute(AgentAction::Prompt{pane_id, session_id: Some(ref.session_id), target_session: (!current).then(session), submission_id})` with `values = [text]`. This is the palette's guarded `Prompt{expected_session_id, wait:false, timeout_ms:30000}`, with `control_agent` for the same session and `prompt_peer` for a peer. |
| `focus` | `agent: AgentRef` | `ScreenInstruction::WebFocusAgent` (5 s timeout) → `{ok:true}`, or `target_changed` (409) / `unavailable` (409, "No terminal attached") |
| `screen` | `agent: AgentRef`, `lines` (1–200, default 80) | `AgentRequest::Read{pane_id, expected_session_id: Some, lines}`. The output is sanitized of control characters other than `\n` and `\t`. |
| `keys` | `agent: AgentRef`, `keys: [..]` (≤ 8, allowlist) | `AgentRequest::SendKeys{pane_id, expected_session_id: Some, keys}` |

- **Peer routing.** For `prompt`, `screen` and `keys`, the target `session` must be present in the fresh snapshot. That also validates the name before `YINZ_SOCK_DIR.join(session)`, so there is no path traversal. A peer request uses `control_peer(session, req, now + 32 s)`; a same-session request uses `agent_control::control_agent(req, client, senders, data)`.
- **Palette `AgentRequest` for history and prompt ops:**
  - `timezone: ClientTimeZone::from_client(tz)` (`yinz-runtime/src/client_time.rs:42`)
  - `client`
  - `opening: 0`, `step: 0`
  - `source: PaneInfo::default()` (History and Prompt never call `current_source`, `palette_agents.rs:122-127,212`)
  - `active: Arc::new(AtomicBool::new(true))`
  - `values`
- **Key allowlist.** These strings are parsed with `KeyWithModifier::from_str`, the same as `yinz agent send-keys` (`src/agent.rs:550-556`): `esc`, `enter`, `tab`, `up`, `down`, `left`, `right`, `backspace`, `y`, `n`, `1`–`9`. A unit test checks that each one parses.
- **Response payloads** (the contents of `ok`):

```jsonc
// list
{"session":"main","now_ms":0,"counts":{"waiting":1,"busy":2,"idle":4},"agents":[{
  "id":"live-agent:…","ref":{"id":"live-agent:…","kind":"claude","session_id":"abc"},
  "session":"main","current_session":true,"pane_id":"terminal_7","pane":"7",
  "provider":"claude","state":"waiting","glyph":"!","status":"needs input","host":null,
  "title":"…","activity":"…","project":"yinz","branch":"main",
  "last_reply":{"text":"…","truncated":false},"last_hook_at_ms":0,"focused":false,
  "selectable":true,"suppressed":false,"directory":"…","run":"claude:abc",
  "wait_needs_a_person":true,"source":"reported","can_prompt":true,"usage_unavailable":null}]}
// history / hit
{"run":"…","retargeted":false,"agent_gone":false,"live":<row>|null,
 "header":{"name":"…","status":"…","location":"…","started":"…","task":"…","project":"…","branch":"…","session_id":"…","details":"…"},
 "events":[{"revision":41,"revisions":[41],"kind":"message","created_at_ms":0,"time":"…","truncated":true,"tool_calls":0,"markdown":"…"}],
 "earlier":true,            // row id "earlier"
 "next_from":57,            // row id "continue" → Run{from}; null otherwise
 "previous_run":null,"origin_run":null,"resumable":false,"status":"…"}
// event / tools
{"header":"message","markdown":"…","tool":"<pretty JSON>"}
// usage
{"usage":<ProviderUsage serde>}
// prompts
{"prompts":[{"text":"…","description":"…"}]}
// prompt / keys / screen / focus
{"response":<AgentResponse serde>}   // screen output is in response.output
```

- `can_prompt` is false when `session_id` is empty. That gives the palette's reason, "Agent has no conversation ID", and the composer is disabled.
- Events are **not** collapsed natively. The client merges pages and collapses tool runs (§6.3), because groups cross page boundaries.
- An `AgentResponse` whose outcome is not OK is still returned under `ok`, so the UI can show it the way the palette does. `TimedOut` or an uncertain error tells the UI to keep the `submission_id` for a safe retry.
- **Error code → HTTP status (mapped in the SDK):**

| Code | Status |
|---|---|
| `invalid` | 400 |
| `not_found` | 404 |
| `target_changed` | 409 |
| `paused`, `unavailable` | 503 (but `unavailable` from `focus` is 409) |
| `failed` | 502 |

### 4.3 Parity checklist

| Palette or panel feature | Web v1 |
|---|---|
| All local sessions, including peers (`peer_sessions_cache`) | Yes (snapshot) |
| Sort: Waiting, Busy, Idle, newest hook, project, branch, pane, session | Yes (`LiveAgentOrder` with the same labels) |
| Row: glyph, provider, status, `@host`, `[pane]`, `project · branch`, activity, `↳ last reply` | Yes (`AgentPresentation`, `agent_title::status`, `compute_labels`) |
| Header: identity, Task, Project, Location, Started, Session | Yes (`header_overlay` and the run header, formatted in the phone's time zone) |
| Quota, with the same unavailability rules, refreshed every 60 s | Yes (`usage`) |
| 30-entry tail, tool collapsing, previews, event and tools readers | Yes |
| Load earlier, continue | Yes (`from` / `next_from`) |
| Search in run, hit anchoring | Yes (`history{query}`, `hit`; UI phase 2 if time runs short) |
| Guarded prompt, including peers; prompt library | Yes |
| Focus (same session or switch session) | Yes |
| Details | Yes (`header.details`) |
| Previous run / Original run / Latest | Yes |
| Resume archived run | **Phase 2.** `HistoryAction::Resume` calls `launch(request, …)`, which needs a source pane and client. It would be added only with an attached client, using that client's active pane as the source. |
| Archived-agents search | Phase 2 (`HistoryAction::Search`, read-only) |
| Extra: screen text, answer keys, interrupt (Esc behind a confirm) | Yes, all guarded by `expected_session_id` |
| Excluded: restart, Ctrl-C, close, spawn | D7 |

---

## 5. Steel SDK and the builtin plugin

### 5.1 Native HTML: `yinz-steel/src/html.rs` (new, WS-E)

Registered in `Runtime::new` the same way as `yinz-markdown-escape` (`yinz-steel/src/lib.rs:307`). It uses Steel custom types, with the `Pattern` type in `agent_model/archive.rs` as the precedent: opaque `HtmlNode` and `TrustedHtml`.

| Function | Semantics |
|---|---|
| `(yinz-h tag attrs child ...)` | Builds an `HtmlNode`. `tag` must match `^[a-z][a-z0-9-]*$`, and `style`, `iframe`, `object`, `embed`, `base` and `form` are refused. `attrs` is a hash; values are strings or numbers (escaped `& < > " '`, always double-quoted), `#t` (bare attribute) or `#f`/`json-null` (omitted). Attribute names must match `^[a-z][a-z0-9-]*$`. `on*`, `style` and `srcdoc` are refused. URL attributes (`href`, `src`, `action`, `formaction`) accept only `/x` (not `//`), `#x`, `https:` and `mailto:`; the scheme check is case-insensitive after trimming and decoding entities. `<script>` takes no children and only `src` (same-origin) and `type`. Void elements take no children. |
| children | A string or number is escaped text. `HtmlNode` and `TrustedHtml` are inserted as they are. A plain list is a fragment and is flattened. `#f` and `json-null` are skipped. Hashes are an error. |
| `(yinz-html-raw string)` | Creates `TrustedHtml`; documented as for trusted input only. |
| `(yinz-html node)` | Renders to a string. Depth cap 64, output cap 4 MiB. |
| `(yinz-html-document head-children body-children [attrs])` | `<!doctype html><html lang="en">…` |
| `(yinz-html-json-script id value)` | `<script type="application/json" id=…>`, with `<`, `>`, `&`, U+2028 and U+2029 written as `\uXXXX`. |
| `(yinz-html-escape s)` | Text escaping. |
| `(yinz-markdown->html s)` | Calls `yinz_tile_markdown::html::render_safe`, input ≤ 1 MiB, and returns `TrustedHtml`. |

**`yinz-tile-markdown/src/html.rs`** (new, WS-D) provides `pub fn render_safe(markdown: &str) -> String`, built on pulldown-cmark with the `html` feature (enabled by WS-A in `yinz-tile-markdown/Cargo.toml`):
- `Event::Html` and `InlineHtml` become escaped text;
- images become their alt text;
- links are kept only for `http:`, `https:`, `mailto:`, relative paths and `#` anchors, otherwise rendered as text;
- kept links get `rel="noopener noreferrer"`;
- no `style` attributes are produced.

### 5.2 SDK router, responses and hooks: `yinz-steel/src/web.scm` (new, WS-E)

It is evaluated in `Runtime::new` after `palette.scm` (`lib.rs:362-364`) and kept to roughly 250 lines.

```scheme
;; registration
(yinz-web-route method pattern handler)       ; method "GET"|"POST"|"*"; segments literal, ":name", trailing "*rest"
                                              ; most specific wins (literal > :param > *rest); re-registering replaces; returns unregister thunk
(yinz-web-assets prefix directory)            ; → HttpAssets; directory "web/static" or "builtin:web/static"
(yinz-web-page path content-type thunk)       ; stores thunk, publishes via HttpPublish now and on every hook change
(yinz-web-dispatch request)                   ; host entry (§3.5); returns #f
;; handler: (lambda (req) response-or-deferred); req gains "params"
;; responses
(yinz-web-json value [status])                ; content-type application/json; charset=utf-8, cache-control no-store
(yinz-web-html node [status])                 ; HtmlNode/TrustedHtml → text/html; charset=utf-8
(yinz-web-text string [status])
(yinz-web-error status code [message])        ; {"error":{"code","message"}}
(yinz-web-redirect path)                      ; 303, same-origin only
(yinz-web-not-modified)                       ; 304
;; request helpers
(yinz-web-query req key [default]) (yinz-web-param req key) (yinz-web-header req name)
(yinz-web-body-json req)                      ; content-type checked; raises → 400 bad_json
;; render hooks
(yinz-web-hook-define name default-proc)      ; declares a hook with its default
(yinz-web-hook-wrap! name proc)               ; proc = (lambda (next . args) …); chains; triggers page republish
(yinz-web-hook name . args)                   ; invokes the chain
(yinz-web-hook-names)
```

**Dispatch semantics:**
1. Match the route. No match gives 404 `{"error":{"code":"not_found"}}`.
2. Run the handler under `with-handler`.
3. Pass the result through `%async-track` (`async.scm:181`).
4. Normalize the response and send `(yinz-command (hash "HttpRespond" (hash "request_id" id "response" (yinz-json-string r))))`.

- A `%web-replied` set (capped at 1024, oldest pruned) guarantees **exactly one** reply per id.
- A raised error or a rejected deferred becomes a 500 `{"error":{"code":"internal"}}`. Details go only to `yinz-log`.

**`yinz-steel/src/agents.scm`** (WS-E): add `(yinz-agent-view request)`, which returns a deferred. It is modelled exactly on `yinz-agent-control` (`agents.scm:111-120`):
- id = `%rpc-prefix ":view:" (%async-id)`;
- sends `AgentView{id, request: json}`;
- listens for `CustomMessage "agent-view-response"`;
- resolves with `reply.ok`, or rejects with `reply.error` (a hash with `code`);
- times out after 40 s with code `unavailable`, using `yinz-sleep` as `rpc.scm` does.

Also add `(yinz-web-agent-error->status err)` for the mapping in §4.2.

### 5.3 Standard app in the SDK: `yinz-steel/src/web_app.scm` (new; WS-E adds the include, WS-F writes the content)

`(yinz-web-agent-app configuration)` registers the standard routes, the default hooks, the assets (`"/static/" "builtin:web/static"`) and the two pages. It lives in the SDK because user plugins cannot `require` builtin modules (`yinz-steel/src/modules.rs:271-300` confines imports to the plugin root). A user override is short:

```scheme
;; ~/.config/yinz/plugins/my-web.scm   — config: web { plugin "file:~/.config/yinz/plugins/my-web.scm" }
(define (load configuration)
  (yinz-web-agent-app configuration)
  (yinz-web-hook-wrap! "agent-row"
    (lambda (next row) (yinz-h "div" (hash "class" "mine") (next row)))))
```

**Default hooks** (a stable, documented contract):

| Hook | Arguments | Returns |
|---|---|---|
| `document` | `ctx {title, styles, scripts, config, body}` | document via `yinz-html-document` |
| `head` | `ctx` | list of nodes (meta viewport `viewport-fit=cover`, theme-color, manifest, icon, CSS) |
| `shell` | `ctx` | body nodes for `/`: `<div id="app">`, a noscript message, `yinz-html-json-script "yinz-config" config`, module script `/static/app.js` |
| `login` | `ctx` | body nodes for `/login`: a form (`autocomplete="current-password"`, a "Remember this device" checkbox, a label field), module script `/static/login.js` |
| `agent-list` | `rows` | rows (view-model filter or decoration; may add `badges: [{text,tone}]`) |
| `agent-row` | `row` | fragment |
| `agent-header` | `{live, header}` | fragment |
| `event-body` | `event` | `(yinz-markdown->html markdown)`, or an escaped `<pre>` for tool kinds |
| `reader` | `{header, markdown, tool}` | fragment (tool JSON in an escaped `<pre>`) |
| `details` | `{details}` | `<pre>` |
| `screen` | `{text}` | `<pre class="screen">` |

`config` = `{"title":"yinz","poll":{"list_ms":3000,"tail_ms":2000,"usage_ms":60000,"screen_ms":2000},"quick_keys":["1","2","3","enter","esc","up","down"]}`.

**Routes** (all authenticated, because native code never forwards unauthenticated requests):

| Method and path | Handler |
|---|---|
| `GET /api/agents` | `list` → `agent-list` hook → each row gets `"html"` from `agent-row` |
| `GET /api/history?agent=&run=&from=&q=&tz=` | `history` → `"header_html"` from `agent-header`, `"details_html"`, and each event's `"html"` from `event-body` |
| `GET /api/hit?run=&revision=&tz=` | as history |
| `GET /api/event?run=&revision=&tz=` | `event` → `"html"` from `reader` |
| `GET /api/tools?run=&revisions=1,2,3&tz=` | `tools` → `"html"` from `reader` |
| `GET /api/usage?agent=` | `usage` |
| `GET /api/prompts` | `prompts` |
| `POST /api/agent/prompt` `{agent, submission_id, text}` | `prompt` |
| `POST /api/agent/focus` `{agent}` | `focus` |
| `POST /api/agent/screen` `{agent, lines}` | `screen` → `"html"` from `screen`. This is a POST because it reaches into the pane; it is refused during quiesce. |
| `POST /api/agent/keys` `{agent, keys}` | `keys` |

Pages: `(yinz-web-page "/" …)` and `(yinz-web-page "/login" …)`, both rendered through `document`. Reads are GET so they keep working during quiesce; mutations are POST.

Steel validation: `revision` and `from` are decimal; `revisions` has at most 30 entries; `submission_id` has a UUID shape and ≤ 64 characters; `text` is non-empty and ≤ 32 KiB. Native code validates again.

### 5.4 Builtin plugin (WS-F)

```
yinz-steel-builtins/assets/plugins/
  web.scm            ; (define (load configuration) (yinz-web-agent-app configuration))  — headless
  web.test.scm
  web/static/app.js  login.js  api.js  timeline.js  views/list.js  views/detail.js
             views/sheets.js  views/ui.js  app.css  icon.svg  manifest.webmanifest
             vendor/htm-preact-standalone.js  vendor/LICENSE-htm  vendor/LICENSE-preact  vendor/VENDOR.md
```

Registration:
- `yinz-runtime/src/consts.rs:46-69`: add `"web"`.
- `yinz-plugin-host/src/steel/unit/host.rs` `expected_builtin_pane`: `"web" => PaneDeclaration::None`.
- `yinz-plugin-host/src/testing/steel/unit.rs:627`: add `scheme_builtin_web_unit => "web"` to the `Unit;` list only.
- The `default.kdl` alias is WS-A's.

---

## 6. Frontend (Preact + hooks + htm, no build step)

### 6.1 Stack and rules

- **Vendor:** `htm@3.1.1` `preact/standalone.module.js` as `vendor/htm-preact-standalone.js`. It is one ESM file exporting `html`, `render`, `useState`, `useEffect`, `useRef` and the rest, with **no bare imports**. Record the version, source URL and sha256 in `VENDOR.md`, alongside both MIT licenses.
  - Fallback: vendor `preact` and `preact/hooks` separately, rewriting the bare `"preact"` import to a relative path, and record that.
- Scripts are referenced only through `<script type="module" src>`. Never use inline scripts, inline styles or `style` attributes; use classes only.
- Server HTML goes into the page through exactly one component, `ServerHtml({html})` in `views/ui.js`, which uses `dangerouslySetInnerHTML`. A unit test greps the static tree to ensure `dangerouslySetInnerHTML` and `innerHTML` appear only there. Any field from agent data that is not a `*html` field is rendered as text.
- Bootstrap: `JSON.parse(document.getElementById('yinz-config').textContent)`.

### 6.2 Screens

- **Login** (`/login`, `login.js`):
  - POSTs to `/api/login` with `{password, label, remember}`.
  - Stores the token in `localStorage` when remembered, otherwise in `sessionStorage`.
  - Redirects to `/` and restores the saved `location.hash` from `sessionStorage`.
  - On 429, shows a `Retry-After` countdown. On 503 `no_password`, shows "Run `yinz set-password` on the host".
- **Agents list** (`#/`):
  - Sticky header: "Agents · N need you", the time since the last update, the serving session, and a menu with Log out and Log out everywhere.
  - Sections in server order: **Needs you** (waiting, amber `#f59e0b`, pulsing `!`, a red edge when `wait_needs_a_person`), **Working** (blue `●`), **Idle** (grey `○`). Each has a text label, so colour is never the only signal.
  - Each card is a tap target of at least 56 px. It contains `ServerHtml(row.html)` plus Preact badges: a peer-session chip when `current_session` is false, `⌖` when `focused`, `@host`.
  - Waiting cards show inline quick keys (`1`, `2`, `Esc`) and Open.
  - The document title becomes `(N) yinz`. Dark mode follows `prefers-color-scheme`.
- **Agent detail** (`#/a/<encodeURIComponent(id)>?run=<run>`):
  - Sticky compact header (`header_html`) with a back button and the quota bar from `/api/usage`.
  - Segmented control: **Timeline | Screen | Details**.
  - **Timeline:**
    - newest at the bottom, pinned there unless the user scrolls up (then a "↓ New" chip appears);
    - "Load earlier" at the top when `earlier` is true;
    - tool groups show "tools ×N" and open the tools reader sheet;
    - tapping a truncated event opens the reader sheet;
    - `agent_gone` shows a banner and disables actions; `retargeted` resets the timeline.
  - **Screen:** `ServerHtml(screen html)`, polled every 2 s while visible, with a quick-keys row. When the agent is waiting, this tab is preselected so the user sees the permission prompt before tapping a key.
  - **Details:** `details_html` plus Previous run / Original run links.
  - **Bottom composer** in the thumb zone:
    - auto-growing textarea (font-size 16 px, so iOS does not zoom), Send, a "Past prompts" sheet from `/api/prompts` with local filtering, safe-area padding;
    - hidden when `live` is null; disabled with the reason when `can_prompt` is false.
  - **Overflow ⋯ bottom sheet:**
    - Focus on desktop (labelled "switches session to X" for peers; 409 shows "No terminal attached");
    - Interrupt (Esc, with a confirm);
    - Send keys…
- **Prompt safety:**
  1. Create `submission_id = crypto.randomUUID()` on the first Send and keep it with the draft in `sessionStorage` (keyed by agent id and `session_id`) until the outcome is definite.
  2. A `Submitted` outcome clears the draft.
  3. `TimedOut`, 504 or a network error shows "Outcome unknown — check the timeline", with **Retry (same submission)** and **Discard**.
  4. A 409 `target_changed` refreshes the view.

### 6.3 Data and polling (`api.js`, `timeline.js`)

- Every request sends `Authorization: Bearer`. GETs get an `AbortController` timeout of 10 s, POSTs 45 s. At most one request per resource is in flight.
- **401:** save the hash and the draft, clear the token, and go to `/login`.
- **429:** a toast with the retry time.
- **503:** `updating` shows a banner "Yinz is updating — retrying"; `starting` or `unavailable` backs off.
- **Network error, 502 or 504:** exponential backoff 2 → 30 s, an "Offline · updated 2m ago" banner, and the stale data kept on screen, dimmed.
- **Cadence:** list every 3 s; detail tail every 2 s; usage every 60 s; screen every 2 s. Polling pauses while `document.hidden` and refreshes immediately on `visibilitychange`. No push in v1.
- **Tail:** start with `from = null`. Each poll uses `from = newest + 1`, and while `next_from` is set the client fetches again immediately. Load earlier uses `from = max(1, oldest − 30)` and follows `next_from` until it reaches `oldest`.
- **Merge:** events go into a `Map<revision, event>`. `collapseTools(events)` is a port of `yinz_bus::command_palette::collapse_tools` (`yinz-bus/src/command_palette.rs:321-345`):
  - consecutive `tool_call` and `tool_result` events become one `{kind:"tools"}` group;
  - groups hold at most 30 revisions, and `tool_calls` are summed;
  - a source comment names the Rust function it mirrors.
- **Installable:** `manifest.webmanifest` with `display: standalone`, the icon and the theme colour. No service worker in v1.

---

## 7. CLI

**`yinz-cli/src/args.rs:442` `Command`:**
```rust
/// Set, clear, or rotate the password for the mobile web interface.
#[clap(group(clap::ArgGroup::new("mode").args(["password_stdin", "clear", "revoke_sessions"])))]
SetPassword {
    #[clap(long)] password_stdin: bool,   // read one line; strip one trailing \n or \r\n; ≤1024 bytes; no NUL
    #[clap(long)] clear: bool,            // delete password and sessions (servers unbind within ~3 s)
    #[clap(long)] revoke_sessions: bool,  // keep password, sign out every device
},
/// Mobile web interface helpers.
#[clap(subcommand)]
Web(Web),
// pub enum Web { Status { #[clap(long)] https_port: Option<u16> } }
```

There is deliberately no `--password <value>` flag: it would appear in `ps` and shell history, and `Command` derives `Debug` and `Serialize`.

| File | Change |
|---|---|
| `src/remote_commands.rs:106-116` | Add `SetPassword` and `Web` to the `Route::Local` arm |
| `src/unit/remote_commands.rs` | Add rows `(vec!["set-password"], Local)` and `(vec!["web","status"], Local)` |
| `src/main.rs` | Two blocks after `Skills` (`:399-405`). Resolve the data dir as `Hooks` does (`opts.data_dir.clone().unwrap_or_else(get_default_data_dir)`), then call `set_password::run` or `web_cli::status` |
| `src/set_password.rs` (new) | See below |
| `src/web_cli.rs` (new) | See below |

**`src/set_password.rs`:**
- Choose the mode with `IsTerminal`.
- On a TTY: `dialoguer::Password::new().with_prompt("New web password").with_confirmation("Confirm password", "Passwords do not match").allow_empty_password(false).interact_on(&Term::stderr())`, wrapped in the termios save/restore and `Interrupted` → cancel handling from `src/remote_session.rs:444-468`.
- Without a TTY and without `--password-stdin`: error.
- Values are `Zeroizing<String>`, minimum 12 Unicode scalar values. Call `yinz_web::auth::password::write`.
- Print:
  ```
  Web password set. All existing web sessions were signed out.
  Enable the server with `web { enabled true }` in config.kdl, then expose it to your tailnet only:
    tailscale serve --bg --https=7443 http://127.0.0.1:7419
  Never use `tailscale funnel` for this port.
  ```
  Take the ports from the loaded config when available.

**`src/web_cli.rs` `status`:**
1. Load the config with the same loader `yinz setup --check` uses. If it fails, show defaults with a warning. Print `enabled`, the bind address and the port.
2. Print the password state: Missing, Ready with `updated_at`, or Invalid with the reason. Also print the active session count, labels and last-seen times.
3. List listeners from `<data_dir>/web/listeners/*.json`, checking each with `kill(pid, 0)` and deleting stale files. Probe `http://127.0.0.1:<port>/healthz` with a hand-written GET over `std::net::TcpStream` to name the session holding the port.
4. Print the serve command using `--https-port`, else `https_port_hint` (7443), plus the expected origin. If `tailscale` is on `PATH`, read `tailscale status --json` (`Self.DNSName`) to print the full URL `https://<dns>:7443/ (tailnet only)`.
5. If `tailscale serve status --json` shows `AllowFunnel` for any handler that targets the backend, print a loud warning and **exit non-zero**. The default 7443 cannot be funneled: Funnel only allows 443, 8443 and 10000, and 443 and 8443 are already used on this machine.

Also update `README.md`, `docs/AGENT_CLI.md` and the hand-written completions in `yinz-cli/assets/completions/{comp.bash,comp.zsh,comp.fish}`.

---

## 8. Test plan

This follows CLAUDE.md: tests go in adjacent `unit/` directories, test runs are selected per change, and full suites run only at merge. Tests that bind TCP or Unix sockets use **sandbox escalation on the first attempt**.

| Area | Tests | Command |
|---|---|---|
| Config (WS-A) | Parse, merge and `to_kdl` round trip; validation (non-loopback without opt-in, bad port, `session_days` range); duplicate block; both editor arms; `default.kdl` and `example/default.kdl` parse | `cargo x test -p yinz-config -E 'test(/web/) \| test(/duplicate/)'` |
| Bus and contexts (WS-A) | `PluginContext::Web`, the screen contexts, `ReplySlot` first-send-wins across clones | `cargo x test -p yinz-bus -E 'test(/instruction_context/) \| test(/web/)'` |
| Wire (WS-A) | The four commands decode from Steel JSON; protobuf refuses them | `cargo x test -p yinz-plugin-api -E 'test(/plugin_command/)'` |
| yinz-web HTTP (WS-B, `unit/{service,policy,request,listener,site}.rs`) | Host table (loopback, `*.ts.net`, a DNS-rebinding host → 421); Funnel header → 403; Origin exact authority (`:443` page vs `:7443` → 403; `X-Forwarded-Host`; `allowed_origins`); `Sec-Fetch-Site`; 415, 413, 408; path traversal and encoding; unauthenticated `/api/*` produces **no** dispatch (fake `Backend` counts calls); site 200/304/503-starting/404; `None` → 503; timeout → 504 (paused clock); quiesced POST → 503; disallowed plugin header → 502; security headers on 200/401/404/413/502/504/site; HEAD has no body; idle keep-alive closed after `header_timeout`; a pre-byte RST and an idle socket do not block other requests (regressions for the vetis bugs); listener: no password → no bind, set password → binds, port held → `Binding` then binds after release (`Watched::wait_until`), quiesce releases, `stop()` frees the port at once | `cargo x test -p yinz-web -E 'not test(/auth::/)'` |
| yinz-web auth (WS-G, `unit/{password,sessions,limiter,endpoints}.rs`) | Round trip; 0600; refusal of a symlink, 0644, a foreign owner, an oversized file, PHC parameters below the floor or above the ceiling; `write` rotates `verifier_id` and empties sessions; sessions expire (absolute, idle with and without `remember`) under an injected clock; `verifier_id` mismatch; eviction at 16; an external rewrite is picked up; limiter per-key, global and lockout doubling; argon2 queue overflow → 429; login JSON statuses | `cargo x test -p yinz-web -E 'test(/auth/)'` |
| Plugin-host seam (WS-C, `unit/web_requests.rs`, `unit/web_assets.rs`, `yinz_exports` unit) | `HttpRespond` correlation (unknown id ignored, duplicate ignored); dispose closes slots (receiver errors); pending cap → `Unavailable`; response validation; asset loader confinement (`..`, absolute path, symlink escape, FIFO, size caps, extension allowlist, `builtin:` root); publish only for the target; restart rate limit; `AgentView` identity gate | `cargo x test -p yinz-plugin-host -E 'test(/web_requests/) \| test(/web_assets/) \| test(/yinz_exports::.*web/)'` |
| Builtin generation (WS-C) | `retained_source` keeps `web/static/*.js` and drops other non-Scheme files; a snapshot after exchange contains the assets | `cargo x test -p yinz-runtime -E 'test(/builtin_generation/) \| test(/fs_beneath/)'` |
| Steel module confinement after the move (WS-C) | Existing module tests | `cargo x test -p yinz-steel -E 'test(/modules/)'` |
| Steel SDK (WS-E, `unit/{html,web}.rs`) | Text and attribute escaping; `JaVaScRiPt:` and entity-encoded schemes refused; `on*`, `style`, `srcdoc` refused; a hash child is an error; list children are fragments; script rules; void elements; JSON-script escaping of `</script>` and U+2028; depth and size caps; route precedence and params; deferred handler replies once; a raising handler gives a generic 500; hook define, wrap and next; `yinz-web-page` republishes on wrap; `Runtime::web` goes through the async lifecycle | `cargo x test -p yinz-steel -E 'test(/html/) \| test(/web/) \| test(/worker/)'` |
| Markdown (WS-D) | Raw HTML escaped; `javascript:` links become text; images become alt text; `rel` attribute present | `cargo x test -p yinz-tile-markdown` |
| Agent view (WS-D, `yinz-plugin-host/src/unit/web_agents.rs`) | List sort matches `LiveAgentOrder` given labels; mapping of `HistoryReply::Run` rows to `earlier`, `next_from`, `previous_run`, `origin_run`, `resumable`; overlay and retarget; `agent_gone`; `target_changed`; peer session not in the snapshot → `not_found`; key allowlist parses; `usage_unavailable`; `paused` | `cargo x test -p yinz-plugin-host -E 'test(/web_agents/) \| test(/palette_agents/) \| test(/palette_history/)'` |
| Screen (WS-D) | `WebLiveAgents` equals the `live_palette_agents` mapping, including peers and exited panes; presentation fields equal `AgentPresentation::from_pane`; `focus_live_agent` with no client, a vanished agent, and a peer; palette regressions | `cargo x test -p yinz-screen -E 'test(/web_agents/) \| test(/palette/)'` |
| Builtin Scheme (WS-F, `web.test.scm`) | Each route's `HttpRespond` JSON via `test-commands`, after feeding `test-event (hash "CustomMessage" (list "agent-view-response" …))`; agent errors map to statuses; unknown route → 404; a raising handler → exactly one 500; `/` and `/login` published with escaped config; a wrapped `agent-row` hook is used; prompt body validation | `cargo x test -p yinz-plugin-host -E 'test(scheme_builtin_web_)'` and `cargo x test -p yinz-plugin-host -E 'test(migrated_builtins_load_through_the_real_host_dispatcher)'` |
| Frontend (WS-F) | `node --test` for `timeline.js` (`collapseTools` parity cases copied from the Rust unit tests) and the `innerHTML` grep, if node is available; otherwise a Rust unit test in `yinz-steel-builtins` that greps the static tree | `cargo x test -p yinz-steel-builtins` |
| CLI (WS-G) | Parse tests (flag group, no value flag); route table; `set_password` stdin mode against a temp data dir | `cargo x test -p yinz -E 'test(/set_password/) \| test(/remote_commands/) \| test(/cli::/)'` |
| Whole app (WS-B, `yinz-integration-tests/tests/web.rs`) | `web_login_and_agents`: write the password into the per-test data dir, start with `web { enabled true; port 0 }`, wait on the listener file, check `/api/agents` 401 → login → 200 JSON, `/` HTML with CSP, `/static/app.js` 200 with ETag, logout → 401. `web_password_rotation_revokes`. `web_port_takeover`: two sessions on a fixed port picked by bind-and-release; the second binds within 10 s of the first being dropped | `cargo x integration-test -E 'test(web_)' -j 4` |

**Not automated:** a manual phone smoke test through a real `tailscale serve --https=7443`, recorded in `docs/WEB.md`. It checks the `Host`, `X-Forwarded-*`, `Tailscale-User-Login` and Funnel headers, iOS Safari storage, and that the port is tailnet-only.

---

## 9. Workstreams

**Order:** WS-A lands first (short; everything compiles with stubs). Then B, C, D, E, F and G run in parallel. WS-B's integration test runs last, after C through F have landed. At merge, follow the repo lifecycle: rebase onto local `main`, run `cargo x ci check`, then fast-forward.

### WS-A: contracts, config, dependencies, stubs

- **Owns:**
  - `yinz-types/src/web.rs` (new), `yinz-types/src/lib.rs`, `yinz-types/src/input/web.rs` (new), `yinz-types/src/input/mod.rs`, `yinz-types/src/data.rs` (4 commands), `yinz-types/src/diagnostics.rs`
  - `yinz-bus/src/web.rs` (new), `yinz-bus/src/lib.rs`, `yinz-bus/src/plugins.rs`, `yinz-bus/src/screen.rs`
  - `yinz-plugin-api/src/{plugin_command.rs, test_generators.rs, unit/plugin_command_tests.rs}`
  - `yinz-plugin-host/src/yinz_exports/permissions.rs`
  - `yinz-config/src/{config.rs, editor.rs, kdl/mod.rs, kdl/web.rs}`, `yinz-config/src/kdl/unit/{duplicate_blocks.rs, web.rs}` and snapshots, `yinz-config/assets/config/default.kdl`, `example/default.kdl`
  - Root `Cargo.toml` (workspace member `yinz-web`; workspace deps hyper, hyper-util, http, http-body-util, argon2, getrandom, base64, zeroize; root package dep on `yinz-web`), `Cargo.lock`, `yinz-web/Cargo.toml`, `yinz-web/src/lib.rs` (module declarations; empty module files for B and G), `yinz-server/Cargo.toml` (dep on `yinz-web`), `yinz-tile-markdown/Cargo.toml` (pulldown-cmark `html` feature)
  - `docs/PLUGIN_PROTOCOL.md`
- **One-line stub delegations, later filled by the named owners:**
  - `yinz_exports.rs` arms → `web::{respond, assets, publish}` (stub file `yinz_exports/web.rs`, filled by C); `AgentView` arm → identity check plus send `PluginInstruction::Web(AgentView)` (final);
  - `#[path = "web_agents.rs"] pub(crate) mod web_agents;` with a stub `handle` returning `{"error":{"code":"unavailable"}}` (filled by D);
  - `yinz-plugin-host/src/lib.rs`: the `plugin_thread_main(…, web: Option<WebPluginParams>)` parameter, the `PluginInstruction::Web` arm calling `web_requests::WebRequests` methods (stub file, filled by C), and the final `AgentView` spawn with `native_tasks` (§3.2);
  - `yinz-server/src/lib.rs:3244`: passes `None` (B wires the real value);
  - `yinz-screen/src/screen/thread_main.rs`: arms calling `self.web_live_agents(reply)` and `self.web_focus_agent(agent, reply)` in a stub `yinz-screen/src/screen/web_agents.rs` (filled by D);
  - `yinz-steel/src/web_app.scm` placeholder and `web.scm` placeholder (E and F fill them).
  - This is a sequential handoff of stub files, not shared ownership. After A, `lib.rs`, `yinz_exports.rs` and `thread_main.rs` belong to nobody else.
- **Provides:** every type in §3.1 and §4.1, the command shapes in §3.4, `WebConfig`.
- **Tests:** the Config, Bus and Wire rows in §8.

#### WS-A as built (read before starting B–G)

The sections above are updated in place; this list collects what differs from the original plan.
- **Config (§1.4):** the listen setting is `address`, not `bind`; `WebConfig` derives `Eq` and gains `validate()`, `session_days_or_default()`, `https_port_hint_or_default()` and the `DEFAULT_WEB_*`/`WEB_SESSION_DAYS_RANGE` constants; the KDL code is the free functions `kdl::web::{from_kdl, to_kdl}`; a repeated scalar setting is an error.
- **Derives on seam types (§3.1):** `WebReply`, `WebResponse`, `StaticFile` and `Site` also derive `Clone, PartialEq, Eq`; `WebAuth` and `WebRequest` add `PartialEq, Eq`; `SiteHandle` and `AgentViewCall` have hand-written `Debug` impls that print no bodies or secrets; `WebLiveHeader` derives `PartialEq, Eq`.
- **Plugin thread (§3.2):** the update-pause check is in the `lib.rs` arms (both `Request` and `Activate`); the paused `AgentView` reply is `{"id", "reply": {"error": {"code": "paused"}}}`; `activate` sets the `WebHost` target before guest initialization through `PluginBridge::start_background_plugin_then`.
- **WebHost (§3.3):** already built; C wires it into `PluginBridge`/`PluginEnv` and deletes each `#[expect(dead_code)]` as it uses the item.
- **Identity gate (§3.4):** C changes `yinz_exports/web.rs` `is_web_plugin`.
- **yinz-web (§1.2):** `DispatchFuture` alias, the `DEFAULT_*` timeout constants, derives on `WebSettings`/`WebStatus`, module visibility, and the auth helper types in §2.1.
- **Steel:** `web.scm` and `web_app.scm` are comment-only placeholders that are not evaluated; evaluating them in `Runtime::new` is E's.
- **Dependencies:** `hyper-util` features `["tokio","server","server-graceful","http1"]`; `http`, `bytes` features `["std"]`; `zeroize` `["alloc"]`; `getrandom` default features off. Lock side effects: `rand_core` 0.6.3 → 0.6.4, and eleven Windows-only crates moved to the already-locked `windows-sys` 0.60.2.
- The alias `yinz:web` is safe before F lands: builtin tags are checked when a plugin loads, not when config is parsed.

### WS-B: HTTP server and server wiring (plus the final integration test and docs)

- **Owns:** `yinz-web/src/{server,listener,service,request,policy,site,pages,listener_file}.rs`, `yinz-web/src/auth/mod.rs` (the stub signatures from §2.1 only, then handed to G), `yinz-web/src/unit/{service,policy,request,listener,site}.rs`; `yinz-server/src/web.rs` (new, `PluginBackend`), `yinz-server/src/lib.rs` (`SessionMetaData.web_server`, start in `init_session`, Drop, the real `WebPluginParams` passed to `plugin_thread_main`); `yinz-integration-tests/tests/web.rs`; `docs/WEB.md` (setup, `tailscale serve` without Funnel, ACL advice, threat model, override guide); the `yinz-web/` row in the CLAUDE.md layout table.
- **Consumes:** `yinz_types::web::*` (A) and the `Auth` API (G).
- **Tests:** the HTTP and whole-app rows in §8.

### WS-C: plugin-host seam, assets, generation fix

- **Owns:** `yinz-plugin-host/src/web_requests.rs`, `web_host.rs`, `web_assets.rs` (all new); `yinz_exports/web.rs`; `plugin_bridge.rs` (`plugin_web`); `plugin_map.rs` (`PluginEnv.web`, `web_pending`, `plugin_web` wrapper, env construction); `steel/mod.rs` (`SteelPlugin::web`, dispose cleanup); `background_worker.rs` (empty `web_pending` for workers, if needed); `testing/steel.rs` (a `"web"` harness verb calling `SteelPlugin::web`) and `testing/steel/bootstrap.scm` (`(define (test-web request) (%test "web" request))`); `yinz-plugin-host/src/unit/{web_requests,web_assets}.rs`; `yinz-runtime/src/builtin_generation.rs` and its unit test; `yinz-runtime/src/fs_beneath.rs` (new) and the `yinz-runtime/src/lib.rs` module line; `yinz-steel/src/modules.rs` (switch to `fs_beneath` only).
- **Consumes:** `WorkerRuntime::web(Value) -> Result<bool>` (E; stub it locally until E lands).
- **Read first:** `yinz-plugin-host/src/steel/CLAUDE.md`.
- **Tests:** the seam, generation and module rows in §8.

### WS-D: native agent view

- **Owns:** `yinz-plugin-host/src/web_agents.rs` (body), `palette_agents.rs` (visibility, `compute_labels`, `control_peer`), `palette_history.rs` (visibility only), `yinz-plugin-host/src/unit/web_agents.rs`; `yinz-screen/src/screen/web_agents.rs` (body), `yinz-screen/src/screen/palette_history.rs` (extract `label_source`, `header_overlay`, `usage_unavailable`, `focus_live_agent`), `yinz-screen/src/screen/command_palette.rs` (use `label_source`), the yinz-screen unit tests for these; `yinz-tile-markdown/src/{lib.rs, html.rs, unit/html.rs}`.
- **Provides:** the §4.2 operations and JSON, and `render_safe`.
- **Tests:** the Markdown, Agent view and Screen rows in §8.

### WS-E: Steel SDK

- **Owns:** `yinz-steel/src/html.rs` (new), `yinz-steel/src/web.scm`, `yinz-steel/src/lib.rs` (register HTML natives, evaluate `web.scm` and `web_app.scm`, `Runtime::web`), `yinz-steel/src/worker/{protocol,server,client}.rs`, `yinz-steel/src/agents.scm` (`yinz-agent-view`), `yinz-steel/src/unit/{html,web}.rs`, `docs/PLUGIN_SDK.md` (Web section), and `yinz-steel/src/sandbox.rs` only if the natives need allowlisting (**VERIFY**).
- **Must:** record plugin startup `phase=` log timings before and after adding the two SDK files.
- **Provides:** the §5.1–5.2 API.
- **Tests:** the Steel SDK row in §8.

### WS-F: builtin app and frontend

- **Owns:** `yinz-steel/src/web_app.scm` (content: routes, default hooks, pages); `yinz-steel-builtins/assets/plugins/web.scm`, `web.test.scm`, `web/static/**` (including vendored code, licenses and `VENDOR.md`); `yinz-runtime/src/consts.rs`; `yinz-plugin-host/src/steel/unit/host.rs`; `yinz-plugin-host/src/testing/steel/unit.rs`; `yinz-steel-builtins/src/unit/*` for the static-tree checks.
- **Consumes:** the E API, the D JSON, and the CSP in §2.6.
- **Develops against:** `test-event` stubs for `agent-view-response` until D lands.
- **Tests:** the builtin Scheme and frontend rows in §8.

### WS-G: auth and CLI

- **Owns:** `yinz-web/src/auth/{mod,password,sessions,limiter,endpoints}.rs` (bodies), `yinz-web/src/unit/{password,sessions,limiter,endpoints}.rs`; `yinz-cli/src/args.rs`, `src/main.rs`, `src/remote_commands.rs`, `src/unit/remote_commands.rs`, `src/set_password.rs`, `src/web_cli.rs`, `src/tests/cli.rs`, `src/unit/set_password.rs`, `yinz-cli/assets/completions/*`, `README.md` and `docs/AGENT_CLI.md` CLI entries.
- **Provides:** the §2.1 API and file formats.
- **Tests:** the auth and CLI rows in §8.

### Interfaces between workstreams

| From → to | Interface |
|---|---|
| A → all | `yinz_types::web::{WebRequest, WebReply, WebResponse, Site, SiteHandle, StaticFile}`, `yinz_bus::web::{ReplySlot, WebInstruction, AgentViewCall, WebPluginParams, WebLiveAgent, WebLiveHeader, WebAgentRef}`, `PluginCommand::{HttpRespond, HttpAssets, HttpPublish, AgentView}`, `WebConfig` |
| G → B | `Auth` API (§2.1), `PasswordStateKind` |
| B ↔ C | `PluginInstruction::Web(Request{request, reply} \| Activate)`; `SiteHandle` shared via `WebPluginParams` |
| C ↔ E | `WorkerRuntime::web(Value) -> Result<bool>`; guest request hash (§3.7); `HttpRespond` JSON (§3.4) |
| C ↔ D | `PluginInstruction::Web(AgentView)` → `yinz_exports::web_agents::handle(&str, ClientId, &ThreadSenders, &DataAccess) -> Value`, delivered as `CustomMessage("agent-view-response", {"id","reply"})` |
| D ↔ F | §4.2 operations and JSON |
| E ↔ F | §5.2 API and the §5.3 hook contexts |
| F ↔ B | Page paths `/`, `/login`; asset prefix `/static/`; frontend calls the native `/api/login`, `/api/logout`, `/api/logout-all`, `/api/session` |

---

## 10. Open risks

| Risk | Detail | Mitigation |
|---|---|---|
| R1 | Unverified: does `tailscale serve` keep `Host`, set `X-Forwarded-Host`/`-For`, set `Tailscale-User-Login`, and use the header name `Tailscale-Funnel-Request`? | The Host check is a union that includes loopback. Origin accepts Host, `X-Forwarded-Host` or `allowed_origins`, and `yinz web status` prints the expected origin. Rejection reasons are logged; they are not secret. Confirm during WS-B's manual smoke test. |
| R2 | A bearer token in browser storage is exposed to XSS. | Strict CSP (no inline script, style or handlers; `connect-src 'self'`; `img-src 'self' data:`), opaque HTML nodes, `render_safe`, one `ServerHtml` sink checked by a grep test, idle expiry, `logout-all`. Optionally add Trusted Types after a browser test. |
| R3 | `web.scm` and `web_app.scm` are evaluated in every plugin worker, which adds startup time. | WS-E measures. If the regression is noticeable, register `web_app.scm` with `engine.register_steel_module` and evaluate it only on the first `yinz-web-agent-app` call. That is a follow-up within WS-E. |
| R4 | The vendored htm standalone bundles an older Preact, and hooks might misbehave. | Fall back to vendoring Preact and hooks separately with rewritten relative imports (§6.1). |
| R5 | `ensure_secure_dir` might reject temp data dirs in integration tests. | WS-G verifies, with the fallback in §2.1. |
| R6 | Named-worker `PluginEnv` construction might not pick up the new fields. | WS-C audits every constructor. Workers get an empty `web_pending`. |
| R7 | Multi-user host: another uid could squat port 7419 while no yinz session holds it. | Documented in `docs/WEB.md`. A v2 option is a 0700 Unix-socket listener, if `tailscale serve` supports Unix targets. |
| R8 | The global login lockout lets an attacker on the tailnet block new logins for 15 minutes. | A deliberate trade for brute-force resistance. Existing sessions are unaffected. |
| R9 | `retained_source` now copies roughly 300 KB of assets into each session's snapshot. | Within the existing limits (4096 entries, 64 MiB). |
| R10 | Resume and archived-agents search are deferred, so parity has a known gap. | Tracked as phase 2 in §4.3. |
| R11 | `web { }` config changes need a session restart in v1. | `yinz web status` shows the effective values. |
