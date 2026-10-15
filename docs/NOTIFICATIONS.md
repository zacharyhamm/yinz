# Notification subscriptions and policies

Bind one policy plugin in your session configuration:

```kdl
hooks {
    notification "file:/absolute/path/to/notification-policy.scm"
}
```

The binding authorizes decisions. Yinz loads one shared headless instance on the
first client attachment and retains it while all clients are detached. See
[`notification-policy.scm`](../example-plugins/notification-policy.scm) for a policy
that rewrites build messages, sends them to both channels, suppresses debug
messages, and redirects desktop notifications to toasts.

Subscribe to `NotificationRequested` in the policy. Reply with
`(yinz-command (hash "ResolveNotification" (hash "id" id "decision" decision)))`.
Decisions are `"Pass"`, `"Suppress"`, or a `"Replace"` hash with optional
`"title"`, `"body"`, and `"delivery"` fields. Omitted fields retain
the original values. Delivery is `"Toast"`, `"Desktop"`, or `"Both"`.
Only the assigned policy instance may resolve a request. Each request has a
one-second deadline including loading; failure, reload, removal, timeout, and
buffer overflow deliver the original content and record a fallback reason.
Oversized replacements also fall back. Input and rendering continue while the
policy works. Outcomes commit in arrival order.

Other plugins can subscribe to `NotificationReceived` and
`NotificationResolved` with `ReadApplicationState`. The first carries the
original notification; the second carries `original`, final `title` and `body`,
`delivery`, `suppressed`, and `fallback_reason`. Arrivals include a session-local
`id`, `received_at_unix_ms`, `source` (`Toast`, `Osc9`, `Osc99`, `Osc777`),
`pane_id`, `plugin_id`, original title/body, and original delivery. Origin fields
are null when unavailable. Notifications emitted by the policy itself bypass
interception while remaining observable. Observers should use origin metadata
to avoid reacting to their own notifications.

`ListNotifications` retains its original `id`, `message`, and
`received_at_unix_ms` fields and oldest-first ordering. Its additive `outcome`
field carries the full resolution, including original metadata. History includes
desktop and suppressed notifications; `message` joins final title and body with
`": "`. Both notification browsers show the source and suppression status.
History activation preserves the originating pane and plugin callback. Suppressed
notifications remain suppressed when activated.

Desktop delivery respects each client's host notification protocol. Rewritten
text is sanitized before rendering or escape encoding. OSC 99 display chunks
are held until completion and decision so suppression hides every chunk. Pass
preserves the retained escapes and existing pane ID namespace. Rewriting keeps
application IDs, activation-report intent, and protocol metadata. Query, alive,
and close requests pass through; close cancels matching pending display work.
The grid retains its 256 tracked IDs per pane. Raw buffering is capped at 64 KiB
per assembly; pending work is capped at 256 notifications and 16 MiB per session.
Overflow forwards original chunks and bypasses interception for the rest of
that assembly.

Detached sessions record and publish outcomes without replaying visible delivery
on reattachment. Agent-state detection uses original content. Plain BEL events
and editing click targets are outside this policy.
