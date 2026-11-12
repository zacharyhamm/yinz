# Reported program status (OSC 7501)

Yinz accepts revision 0.4 of the [Program Status Protocol](https://www.superlogical.com/rex/docs/build/program-status).
Programs can report without installation or a feature query. Queries receive the supported states and blocked kinds, using the request's BEL or ST terminator. Responses travel through the pane PTY, including remote panes; nothing is forwarded to the outer terminal.

Records contain idle, working, done, blocked, or error, plus optional progress, blocked reason, message, title, app and hierarchical task ID. Clear removes a subtree, or all records without an ID. Reports replace records completely. Messages are plain single-line UTF-8 encoded with standard base64; padding is optional. Invalid reports are ignored atomically. Yinz follows the protocol byte limits and retains at most 256 records per pane, evicting the least recently updated.

Records belong to the pane, across alternate-screen switches and semantic pane transfers. RIS clears them; soft reset does not. Shell prompt markers and process exit remove transient records. Done/error remain available as historical results. Child blocked/error records may raise attention over the root; child completion does not complete the root. Working children remain separate records, not proof that parent execution finished.

## Display and fallback

Pane manifests and agent status responses expose `program_status.records` and `program_status.current`. A current applicable report takes precedence in agent frames, palette, agents panel and CLI display. The underlying `agent` remains the provider/identity record. A reported app name never creates or changes an agent conversation. Explicit app names incompatible with the known agent do not override its display.

Hooks continue updating the fallback while OSC is current. Clear restores fallback immediately. New submitted input, correlated provider submission/completion boundaries, conversation replacement, a foreground program change, shell prompt or exit retires old applicability. A later report can establish current status again without reactivating historical records. No heartbeat or silence timeout is required. A process name alone cannot distinguish every same-command restart; conversation hooks and shell markers remain useful, particularly over SSH.

After adopting a local Claude pane without lifecycle tracking, Yinz observes its process-scoped session registry to reconcile the retained display state without another prompt or hook. New input, provider reports, or process/conversation replacement supersede that observation. Unreadable registry data leaves the state unchanged. This observation does not establish readiness or complete submissions; transferred lifecycle and background obligations retain their existing authority.

Do not use reported idle/done/error as automation completion evidence. They do not acknowledge a submission, identify a turn, establish accepted evidence, release a permission owner, prove prompt readiness, or establish that background editing has stopped. Native provider events, conversation/turn IDs, transcript evidence, input revisions, worker tracking and retained completion proofs remain necessary. Existing prompt/wait and lifecycle automation continue using those paths. Basic hook/activity status is fallback; correlation is not replaced.

OSC status changes do not synthesize provider lifecycle events or desktop notifications. Existing hook and notification delivery remains active, avoiding a second notification for the same agent event.

## Compatibility evidence

Inspection on 2026-11-11 used Zellij commit `c8585459c37186fae1321727ad8dd4c96480ef41`, including its parser and record tests. Yinz's local upstream ref predates this protocol; it was not changed.

- Codex source `e58bf8f5233b51e7cd1679d947e3028c1ed91b06` has idle/working/done/error and permission/question/auth reports, with terminal capability/environment gating. These reports carry no Yinz submission identity.
- Pi source `b22e525b325ff1934b4110f7fc60426fd6f0a8f3` supports root records, reports dialogs and compaction, and queries terminal support. Do not assume child records or progress.
- [Zellij's integration matrix](https://zellij.dev/documentation/inbox-integrations.html) lists Claude Code 2.1.295+, Codex 0.163+ (richer states in 0.164), Pi 1.1+, and OpenCode 2.1+. OpenCode is listed as lacking done/error. These version claims are matrix evidence, not a guarantee for an installed executable; other providers and versions remain unverified here.

Support is determined from actual valid reports, never solely from agent name or advertised version. Keep hooks enabled for older agents, disabled terminal reporting, partial implementations, remote environments that suppress reporting, and fields the protocol does not supply.
