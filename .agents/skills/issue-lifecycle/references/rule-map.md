# Lifecycle rule preservation map

Baseline: commit `c3ec8c1920582d2bec184b953e4dfb009b391e78`.
This is a review artifact, not an operating procedure or runtime authority.
Source line numbers always refer to that commit, not the reorganized files:

- `SKILL.md` means `.agents/skills/issue-lifecycle/SKILL.md`.
- `ISSUE_LIFECYCLE.md` means `docs/ISSUE_LIFECYCLE.md`.

Each row names a rule or related clauses and links its authoritative destination.
Repeated source occurrences may converge. The skill owns normal operation/resume
routing; recovery owns conditional blocker procedures; the guide owns design and
contract facts. Unchanged planning/review/dimensions references retain authority.
Operator commands removed from the guide route to the skill rather than another
competing walkthrough.

The skill frontmatter and its activation scope are unchanged.

## Skill clauses

| ID | Baseline source | Rule label | Authoritative destination |
| --- | --- | --- | --- |
| S001 | SKILL.md:12–16 | Agenda requirements, Swamp evidence, Git worktrees and visible reusable workers | [Skill: purpose](../SKILL.md) |
| S002 | SKILL.md:15–18 | Lifecycle behavior and entry workflow identities; detailed reference routing | [Design: stages/evidence](../../../../docs/ISSUE_LIFECYCLE.md#stages-and-evidence), [Skill: blocker routing](../SKILL.md#blocker-routing) |
| S003 | SKILL.md:22–25 | One canonical control checkout and launcher for every operation | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S004 | SKILL.md:24–25 | Different launcher copy selects control explicitly; no per-worktree stores | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S005 | SKILL.md:27–29 | Start in intended feature worktree/subdirectory; create it first if needed | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S006 | SKILL.md:28–31 | Canonical saved worker root; refuse main and another issue's claim | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| S007 | SKILL.md:31–33 | Permanent saved claim; reclaim only an unsaved start claim | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| S008 | SKILL.md:33–36 | Saved directory across every role, revision, resume, restart and rollback | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| S009 | SKILL.md:34–36 | Later invocation directory or repeated start cannot replace worker origin | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| S010 | SKILL.md:36–38 | Pending configuration and control storage retained; legacy origin not inferred | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims), [Design: control checkout](../../../../docs/ISSUE_LIFECYCLE.md#one-control-checkout) |
| S011 | SKILL.md:40–41 | Installed commands, unfamiliar-flag help and machine JSON | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S012 | SKILL.md:41–45 | Vault workspace lookup and missing-value configuration in intended workspace | [Design: workspace/prerequisites](../../../../docs/ISSUE_LIFECYCLE.md#workspace-configuration-and-prerequisites) |
| S013 | SKILL.md:45–46 | No current-selection inference; workspace flag is assertion, not override | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S014 | SKILL.md:46–48 | Workflow/integration independently verify recorded workspace | [Design: workspace/prerequisites](../../../../docs/ISSUE_LIFECYCLE.md#workspace-configuration-and-prerequisites) |
| S015 | SKILL.md:48–49 | Mismatch means inspect configuration, not recreate issue | [Recovery: inspect blocker](recovery.md#inspect-the-blocker) |
| S016 | SKILL.md:49 | Workspace remains public issue identity in state | [Design: workspace/prerequisites](../../../../docs/ISSUE_LIFECYCLE.md#workspace-configuration-and-prerequisites) |
| S017 | SKILL.md:50–51 | Read workspace and item before mutation; use returned item ID | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S018 | SKILL.md:51–52 | Launcher computes hashed long-workspace behavior names | [Design: control checkout](../../../../docs/ISSUE_LIFECYCLE.md#one-control-checkout) |
| S019 | SKILL.md:54–56 | Inspect persisted state and resume; no recreation, history reset or conversational success | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S020 | SKILL.md:56–57 | Title/note requirements; refresh and review changed requirements | [Skill: identity/setup](../SKILL.md#identity-and-setup) |
| S021 | SKILL.md:61–64 | Drive agent input shape, intended running session/provider, omitted defaults | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S022 | SKILL.md:64 | Saved settings; no-input ordinary resume | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S023 | SKILL.md:65–67 | Control session/pane capture and current-tab separate worker stack | [Design: pane protocol](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks) |
| S024 | SKILL.md:66–67 | Revisions, repairs and restarts use controller stack | [Design: pane protocol](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks) |
| S025 | SKILL.md:67–69 | Existing workers keep original tab/submission; session must match | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S026 | SKILL.md:69 | Missing panes/lookup failures block launch | [Recovery: dialogs/hooks/sessions](recovery.md#dialogs-hooks-and-sessions) |
| S027 | SKILL.md:70–72 | Explicit control outside pane environment, or saved identity | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S028 | SKILL.md:72–75 | Legacy driver acquires identity; unlaunched legacy task requires restart | [Recovery: dialogs/hooks/sessions](recovery.md#dialogs-hooks-and-sessions) |
| S029 | SKILL.md:74–75 | No automatic-tab placement bypass | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S030 | SKILL.md:76–78 | Drive after preparation, explicit restart and completed rollback | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S031 | SKILL.md:78–80 | Driver supplies prompts/visible workers; no headless or inline substitute | [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership), [Design: stages/evidence](../../../../docs/ISSUE_LIFECYCLE.md#stages-and-evidence) |
| S032 | SKILL.md:80–81 | Single-role diagnostics; Yinz agents skill for inspection/dialogs | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S033 | SKILL.md:83–85 | Foreground wait on the same yielded command until structured handoff | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S034 | SKILL.md:85–86 | Launch/workflow/progress is not request completion | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S035 | SKILL.md:86–88 | Present issue/version/commit, decision, plan or change evidence and review/checks | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S036 | SKILL.md:89 | Blocker presentation explains cause and recovery | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S037 | SKILL.md:89–90 | No promised wakeup after controller turn/session ends | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S038 | SKILL.md:92–94 | Initial triage/planning/review plus at most one automatic targeted revision/review | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| S039 | SKILL.md:94–96 | Reserve exact assignment prelaunch once per requirements generation | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S040 | SKILL.md:95–96 | Resume/rollback/restart/manual planning/reinvocation cannot replenish allowance | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S041 | SKILL.md:96 | Only genuinely changed requirements on refresh reset generation allowance | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S042 | SKILL.md:97–98 | Legacy first plan may revise; later legacy versions consumed; saved authorizations valid | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S043 | SKILL.md:100–103 | Consumed allowance with high/critical findings yields revision handoff and history | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| S044 | SKILL.md:102–106 | Source-backed adjudication input: version, fingerprint, finding, disposition, rationale, evidence | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S045 | SKILL.md:104–106 | Allowed dispositions and corrected lower severity for downgrade | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S046 | SKILL.md:106–108 | Explain inspected code/judgment; supported controller dispositions need no extra permission | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S047 | SKILL.md:107–108 | Original review retained | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S048 | SKILL.md:108–109 | Accepted unresolved high/critical risk stays blocked for user direction | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S049 | SKILL.md:109–111 | Stale/unresolved-assignment mutation fences; drive after adjudication | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S050 | SKILL.md:113–116 | Concrete handoff and one explicit targeted-revision authorization with exact ID/actual request | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S051 | SKILL.md:116–117 | Save complete assignment/request before launch; reuse on resume then review | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S052 | SKILL.md:117–118 | No blanket approval or repeated permission for authorized assignment | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S053 | SKILL.md:120–123 | Planner/reviewer planning/review references; applicable dimensions for plan/code review | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S054 | SKILL.md:123 | Preserve unaffected steps, accepted tradeoffs and finding IDs | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S055 | SKILL.md:124–125 | Reopening requires new evidence explaining why disposition no longer holds | [Review standards](review.md) |
| S056 | SKILL.md:125–126 | History cannot replace current review evidence | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S057 | SKILL.md:128–131 | Revision approval separate from implementation/merge; driver grants no approvals/CI waiver | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S058 | SKILL.md:129–130 | Separate reviewed-plan approval and worktree preparation before implementation/review | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S059 | SKILL.md:133–134 | Triage source investigation: problem, scope, criteria, reproduction | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S060 | SKILL.md:135–138 | Plan concrete steps/files/risks, focused tests and verbatim argv checks covering steps | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S061 | SKILL.md:138–139 | Present checks; plan approval authorizes their execution | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S062 | SKILL.md:140–141 | One automatic revision; further revisions individually authorized; each freshly reviewed | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S063 | SKILL.md:142–144 | Approve reviewed plan and adopt recorded worktree/existing branch | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S064 | SKILL.md:143–145 | Conflicting path refused before preparation | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| S065 | SKILL.md:145–146 | Existing session approval suffices; missing approval requires concrete presentation | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S066 | SKILL.md:147–149 | Implementation worker implements/commits and runs targeted tests | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S067 | SKILL.md:148–150 | Defects/flakes fixed here through worker; real isolated target and no shared artifacts | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S068 | SKILL.md:151–153 | Code review of committed tree; import validates original task, plan and commit | [Design: stages/evidence](../../../../docs/ISSUE_LIFECYCLE.md#stages-and-evidence) |
| S069 | SKILL.md:153 | Repairs need fresh review | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S070 | SKILL.md:154–156 | Clean review runs plan checks at exact reviewed commit outside merge lock | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S071 | SKILL.md:156 | Record each check exit code, duration and log | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification) |
| S072 | SKILL.md:156–158 | Show delivered change/checklist; reviewer checks advisory | [Skill: drive/resume](../SKILL.md#drive-and-resume), [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S073 | SKILL.md:158–160 | Failed check blocks with log; fix with newly reviewed commit | [Recovery: review/verification](recovery.md#review-and-verification-failures) |
| S074 | SKILL.md:159–160 | No automatic failed-verification rerun; explicit verify only for same-commit environment failure | [Recovery: review/verification](recovery.md#review-and-verification-failures) |
| S075 | SKILL.md:161–163 | Ready requires passing exact-commit verification and actual merge authorization | [Skill: drive/resume](../SKILL.md#drive-and-resume), [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S076 | SKILL.md:162–163 | Existing explicit merge request counts; plan approval is not merge approval | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S077 | SKILL.md:163–164 | Legacy no-check plans keep and disclose reviewer-reported gate | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification) |
| S078 | SKILL.md:165–167 | Control-checkout merge uses coordinator/repository lock across rebase/full CI/FF | [Skill: drive/resume](../SKILL.md#drive-and-resume), [Design: integration boundary](../../../../docs/ISSUE_LIFECYCLE.md#parallel-development-and-serialized-integration) |
| S079 | SKILL.md:167–170 | Full merge CI unless explicit scoped skipCiRequest; time/earlier checks/judgment insufficient | [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| S080 | SKILL.md:171–173 | Compare original requirements; close only after merge and met outcome; unmet stays open | [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| S081 | SKILL.md:175–177 | Launcher transitions; human Agenda descriptions; Swamp evidence/history | [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership) |
| S082 | SKILL.md:179–181 | Direct evidence cannot shortcut workers; operator actions remain controller-owned | [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership) |
| S083 | SKILL.md:185–189 | Independent worktrees; shared-Git lock spans rebase/gate/update; not FIFO or per-issue lock | [Design: integration boundary](../../../../docs/ISSUE_LIFECYCLE.md#parallel-development-and-serialized-integration) |
| S084 | SKILL.md:191–193 | Failure/interruption not completion; inspect state/exact run/Git; retain worktree | [Recovery: inspect blocker](recovery.md#inspect-the-blocker) |
| S085 | SKILL.md:193–194 | Resolve conflicts/check failures here; record repair and fresh review before integration | [Recovery: integration failures](recovery.md#integration-failures) |
| S086 | SKILL.md:195–198 | Present failed merge summary/phases/tests/log; read log and pass cause to repair | [Recovery: integration failures](recovery.md#integration-failures) |
| S087 | SKILL.md:198–200 | Flake/timeout defect; retain mergeFailures and receipt previousAttempts | [Recovery: integration failures](recovery.md#integration-failures) |
| S088 | SKILL.md:200–202 | No silent rebase abort, resets, branch deletion, forced merge or held-lock removal | [Recovery: integration failures](recovery.md#integration-failures) |
| S089 | SKILL.md:204–206 | Unresolved tasks block mutations/replacements; resume original/partial submission | [Recovery: unresolved tasks](recovery.md#unresolved-tasks) |
| S090 | SKILL.md:206–208 | Acknowledged timeouts observed; dialogs, uncertainty, missing session/provider/evidence block | [Recovery: unresolved tasks](recovery.md#unresolved-tasks), [Skill: blocker routing](../SKILL.md#blocker-routing) |
| S091 | SKILL.md:208–210 | Deliberately resolve then drive; low-level resume, never automatic agent replay | [Recovery: unresolved tasks](recovery.md#unresolved-tasks) |
| S092 | SKILL.md:210–211 | Completed handoffs persist; rollback/restart invalidate superseded ones | [Recovery: unresolved tasks](recovery.md#unresolved-tasks) |
| S093 | SKILL.md:212–213 | Save accepted results before close; retry cleanup without replay | [Recovery: imports/cleanup](recovery.md#interrupted-imports-and-cleanup) |
| S094 | SKILL.md:213–215 | Failed/blocked panes visible; missing/stale Codex/Muse/AGY hooks not auto-installed | [Recovery: dialogs/hooks/sessions](recovery.md#dialogs-hooks-and-sessions), [Recovery: imports/cleanup](recovery.md#interrupted-imports-and-cleanup) |
| S095 | SKILL.md:217–219 | Explicit restart/rollback supersedes any outcome without terminal proof or new permission | [Skill: explicit rollback/restart](../SKILL.md#explicit-rollback-or-restart) |
| S096 | SKILL.md:220–223 | Rollback step/request; preserve prerequisites, invalidate downstream; all nine steps | [Skill: explicit rollback/restart](../SKILL.md#explicit-rollback-or-restart) |
| S097 | SKILL.md:223–227 | Restart uses guarded current task ID, actual request, ordinary options, running session | [Skill: explicit rollback/restart](../SKILL.md#explicit-rollback-or-restart) |
| S098 | SKILL.md:226–227 | Identical restart resumes replacement; new restart targets its new ID | [Skill: explicit rollback/restart](../SKILL.md#explicit-rollback-or-restart) |
| S099 | SKILL.md:229–231 | History/Git/ownership preserved, completed tasks reopened, plan versions increasing | [Design: rollback persistence](../../../../docs/ISSUE_LIFECYCLE.md#rollback-persistence) |
| S100 | SKILL.md:230–231 | Landed merge never undone; merge/close-out retries reuse receipt | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| S101 | SKILL.md:231–232 | Changed requirements require triage rollback | [Skill: explicit rollback/restart](../SKILL.md#explicit-rollback-or-restart) |
| S102 | SKILL.md:232–234 | Old panes visible; inspect/stop survivors before same-checkout edits; supersession is not stop | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| S103 | SKILL.md:234–236 | Interrupted rollback resumes recorded intent/replacement without renewed permission | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| S104 | SKILL.md:236–238 | New explicit rollback may supersede pending recovery; preserve intent, no finish prerequisite | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| S105 | SKILL.md:240–242 | Discard unusable completed result only with actual request, confirmedStopped, no background tasks and unapplied evidence | [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures) |
| S106 | SKILL.md:242–245 | Structured rejected launch needs no retained pane/submission/background/resources; original snapshot | [Recovery: rejected launches](recovery.md#rejected-launches) |
| S107 | SKILL.md:245–247 | Explicit new agent corrects rejected config; no automatic config change/relaunch | [Recovery: rejected launches](recovery.md#rejected-launches) |
| S108 | SKILL.md:247–248 | Malformed/transport/legacy uncertain evidence cannot discard; explicit restart/rollback may supersede | [Recovery: uncertain delivery](recovery.md#uncertain-delivery) |
| S109 | SKILL.md:249–250 | Discard cannot stop work/undo changes; inspect before replacement | [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures) |
| S110 | SKILL.md:252–254 | Reconcile saved transition/final task write without duplicate transition/event | [Recovery: imports/cleanup](recovery.md#interrupted-imports-and-cleanup) |
| S111 | SKILL.md:254–257 | Reconcile before refresh; unapplied/implementation evidence still needs fresh requirements | [Recovery: imports/cleanup](recovery.md#interrupted-imports-and-cleanup) |
| S112 | SKILL.md:259–260 | Landed merge close-out retry uses receipt, not remerge | [Recovery: close-out](recovery.md#close-out-failures) |
| S113 | SKILL.md:260–262 | Adopted ad-hoc work needs real plan/review/verification; no fabricated evidence | [Recovery: review/verification](recovery.md#review-and-verification-failures) |
| S114 | SKILL.md:264–265 | No remote Git/PR/deployment/notification actions without their own instructions | [Design: integration boundary](../../../../docs/ISSUE_LIFECYCLE.md#parallel-development-and-serialized-integration) |

## Supplementary guide clauses

These include guide-only safeguards and protocol/reference details; repeated
operator rules are included where needed to audit removal from the guide.
Illustrative shell variables and sample plans were replaced by their input
contracts, not preserved as additional required steps.

| ID | Baseline source | Rule label | Authoritative destination |
| --- | --- | --- | --- |
| D001 | ISSUE_LIFECYCLE.md:42–45 | Existing/search-before-duplicate capture; concise problem/criteria/reproduction note; returned category IDs | [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership) |
| D002 | ISSUE_LIFECYCLE.md:49–61 | Control contents, launcher default/absolute path/control override, same repo and 64-character names | [Design: control checkout](../../../../docs/ISSUE_LIFECYCLE.md#one-control-checkout) |
| D003 | ISSUE_LIFECYCLE.md:63–70 | Direct start workflow requires canonical worker root; claim registry; reject detached HEAD and claimed branch | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| D004 | ISSUE_LIFECYCLE.md:70–76 | Pending-to-permanent claim; reclaim only inactive unsaved start; own retry replacement and confirmation | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| D005 | ISSUE_LIFECYCLE.md:88–101 | Agenda-returned identity; default local encryption vault creation/storage/fresh-clone configuration | [Design: workspace/prerequisites](../../../../docs/ISSUE_LIFECYCLE.md#workspace-configuration-and-prerequisites) |
| D006 | ISSUE_LIFECYCLE.md:103–110 | Every-operation vault lookup, no fallback; independent direct-workflow/integration checks twice; no migration | [Design: workspace/prerequisites](../../../../docs/ISSUE_LIFECYCLE.md#workspace-configuration-and-prerequisites) |
| D007 | ISSUE_LIFECYCLE.md:112–117 | Installed local behaviors, Python 3.11+, PATH/build/provider prerequisites, explicit session | [Design: workspace/prerequisites](../../../../docs/ISSUE_LIFECYCLE.md#workspace-configuration-and-prerequisites) |
| D008 | ISSUE_LIFECYCLE.md:119–120 | Issue on different local branch in same repository; exactly one clean main worktree | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| D009 | ISSUE_LIFECYCLE.md:122–151 | Read workspace/item, start intended worker, inspect status, select drive session/provider via JSON file | [Skill: identity/setup](../SKILL.md#identity-and-setup), [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| D010 | ISSUE_LIFECYCLE.md:154–160 | Issue-scoped evidence; argument-safe text; installed help and direct calls' control repo-dir | [Design: workspace/prerequisites](../../../../docs/ISSUE_LIFECYCLE.md#workspace-configuration-and-prerequisites), [Design: control checkout](../../../../docs/ISSUE_LIFECYCLE.md#one-control-checkout) |
| D011 | ISSUE_LIFECYCLE.md:169–180 | Controller-only deterministic operations; foreground presentation; stderr/stdout and handoff resource fields | [Design: stages/evidence](../../../../docs/ISSUE_LIFECYCLE.md#stages-and-evidence), [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership), [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| D012 | ISSUE_LIFECYCLE.md:182–188 | Atomic assignment/allowance reservation; changed-requirements generation; monotonic versions | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| D013 | ISSUE_LIFECYCLE.md:190–192 | Revision handoff carries original/effective review/history/responses/version; repeated drive reuses it | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| D014 | ISSUE_LIFECYCLE.md:209–216 | Disposition evidence/severity/judgment; exact fingerprint/generation and unresolved-reservation fences | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication), [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D015 | ISSUE_LIFECYCLE.md:217–218 | Retained review preserves applicable adjudication; changed/removed review cannot reuse it | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| D016 | ISSUE_LIFECYCLE.md:231–237 | Persist full actual authorization before dispatch; stale/type/blank rejection; manual rollback not allowance reset | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| D017 | ISSUE_LIFECYCLE.md:237–238 | Restart user instructions reach replacement; accumulated feedback survives revisions | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| D018 | ISSUE_LIFECYCLE.md:240–248 | Planning context/history isolated from current evidence; per-blocker responses and current-plan review | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication), [Planning standards](planning.md), [Review standards](review.md) |
| D019 | ISSUE_LIFECYCLE.md:246–251 | Planning/review/dimensions references and existing-field finding citation convention | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| D020 | ISSUE_LIFECYCLE.md:253–258 | Optional-field compatibility; saved prompts/snapshots; current output contract and legacy allowance | [Design: generations/adjudication](../../../../docs/ISSUE_LIFECYCLE.md#planning-generations-and-adjudication) |
| D021 | ISSUE_LIFECYCLE.md:260–271 | Separate approval/preparation/merge/waiver gates, check-based checklist and repair/fresh review | [Skill: drive/resume](../SKILL.md#drive-and-resume), [Skill: review/authorization](../SKILL.md#review-and-authorization) |
| D022 | ISSUE_LIFECYCLE.md:169–171, 284–309, 549 | Launcher/direct method and workflow identities, verify workflow, repair reason and controller ownership | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs), [Skill: drive/resume](../SKILL.md#drive-and-resume), [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership), [Skill: blocker routing](../SKILL.md#blocker-routing) |
| D023 | ISSUE_LIFECYCLE.md:310–313 | Shared coordinator calls integrate; coordinator and OS Git lock have distinct purposes | [Design: integration boundary](../../../../docs/ISSUE_LIFECYCLE.md#parallel-development-and-serialized-integration) |
| D024 | ISSUE_LIFECYCLE.md:315–318 | Changed descriptions refresh; changed approach revises; new plan/repaired code needs current real evidence | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D025 | ISSUE_LIFECYCLE.md:320–323 | Existing scope-bound approval; present work when absent; implementation does not authorize merge | [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership), [Skill: drive/resume](../SKILL.md#drive-and-resume) |
| D026 | ISSUE_LIFECYCLE.md:327–333 | Captured control identity; current-tab stack-for, shared controller stack, focus preservation | [Design: pane protocol](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks) |
| D027 | ISSUE_LIFECYCLE.md:335–343 | Direct control requirement; older-driver identity without provider replacement; saved identity/legacy restart | [Design: pane protocol](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks) |
| D028 | ISSUE_LIFECYCLE.md:345–350 | Provider kinds, input fields, 120-second startup/600-second observation defaults; no automatic bypass | [Design: pane protocol](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks) |
| D029 | ISSUE_LIFECYCLE.md:352–363 | Saved role cwd, preparation prerequisites, path rejection, branch ownership and clean full committed HEAD | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims), [Design: stages/evidence](../../../../docs/ISSUE_LIFECYCLE.md#stages-and-evidence) |
| D030 | ISSUE_LIFECYCLE.md:365–372 | Legacy directory/preparation unchanged; boundary experiment superseded and not a gate | [Design: worker identity/claims](../../../../docs/ISSUE_LIFECYCLE.md#worker-identity-and-claims) |
| D031 | ISSUE_LIFECYCLE.md:374–378 | Hooks status preflight, guarded readiness, pinned conversation and Muse first-prompt binding | [Design: pane protocol](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks) |
| D032 | ISSUE_LIFECYCLE.md:378–379 | Dialogs deliberate; timeout not authority to answer | [Recovery: dialogs/hooks/sessions](recovery.md#dialogs-hooks-and-sessions) |
| D033 | ISSUE_LIFECYCLE.md:381–384 | Task-before-dispatch, intent-before-spawn and ID-before-prompt persistence; unresolved-task exclusion | [Design: pane protocol](../../../../docs/ISSUE_LIFECYCLE.md#pane-tasks) |
| D034 | ISSUE_LIFECYCLE.md:395–402 | Background final-idle observation; correlated cleanup, visible failed/superseded panes and legacy cleanup | [Recovery: imports/cleanup](recovery.md#interrupted-imports-and-cleanup), [Recovery: unresolved tasks](recovery.md#unresolved-tasks) |
| D035 | ISSUE_LIFECYCLE.md:404–406 | Missing child initial state resumes original under child lock; existing child validated | [Recovery: unresolved tasks](recovery.md#unresolved-tasks) |
| D036 | ISSUE_LIFECYCLE.md:408–417 | Discard exact request/stopped confirmation/identity/snapshot, completed-no-background or resource-free rejection | [Recovery: rejected launches](recovery.md#rejected-launches), [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures) |
| D037 | ISSUE_LIFECYCLE.md:416–422 | Resume/discard cannot alter configuration or launch replacement; no active/uncertain discard, close or undo | [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures), [Recovery: rejected launches](recovery.md#rejected-launches) |
| D038 | ISSUE_LIFECYCLE.md:423–427 | Terminal acknowledged failed/StopFailure discard rechecks submission and fresh correlated idle/no-background status | [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures) |
| D039 | ISSUE_LIFECYCLE.md:429–435 | Final prompt-preparation failure: structured not_sent, full correlation, failure/no ack/turn/background | [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures) |
| D040 | ISSUE_LIFECYCLE.md:434–437 | Fresh same-worker idle, no clearing/submission writes; clearing_draft/pending_submission block; actual confirmation/snapshot | [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures) |
| D041 | ISSUE_LIFECYCLE.md:439–443 | Reason and distinct failure/status retained; draft allowed/inspect; no resend/respawn/close/success; explicit replacement | [Recovery: unusable/stopped results](recovery.md#unusable-evidence-and-stopped-failures) |
| D042 | ISSUE_LIFECYCLE.md:445–446 | Optional delivery missing means unknown; text is not non-delivery evidence | [Recovery: uncertain delivery](recovery.md#uncertain-delivery) |
| D043 | ISSUE_LIFECYCLE.md:447–449 | Started before any prompt/prefix write, irreversible; marker/clearing writes excluded | [Recovery: uncertain delivery](recovery.md#uncertain-delivery) |
| D044 | ISSUE_LIFECYCLE.md:449–453 | Unknown/mismatched/active/uncertain discard forbidden; updated server/behavior required, legacy explicit supersession | [Recovery: uncertain delivery](recovery.md#uncertain-delivery) |
| D045 | ISSUE_LIFECYCLE.md:455–461 | Applied import reconciles despite changed Agenda; unapplied/implementation evidence still freshness-checked | [Recovery: imports/cleanup](recovery.md#interrupted-imports-and-cleanup) |
| D046 | ISSUE_LIFECYCLE.md:465–469 | Explicit request supersedes running/missing/expired/uncertain/malformed; no terminal proof; ordinary timeout no restart | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| D047 | ISSUE_LIFECYCLE.md:471–482 | All rollback steps, prerequisites, downstream invalidation, triage for changed requirements, reopened descriptions | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| D048 | ISSUE_LIFECYCLE.md:484–490 | Archive partial/failure evidence; new rollback ID fences same-phase results; no reset/abort/undo/close; stop survivors | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| D049 | ISSUE_LIFECYCLE.md:492–496 | Check durable Git receipt under lock; retain landed merge/history; earlier rollback needs new review/authorization | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| D050 | ISSUE_LIFECYCLE.md:498–515 | Ordinary step after rollback; restart options/current-task guard/new identities/idempotent replacement | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| D051 | ISSUE_LIFECYCLE.md:517–526 | Pre-mutation recovery intent; latest status; interrupted authorized intent resume; pending/new-intent/history resources | [Recovery: rollback/restart](recovery.md#rollback-and-restart) |
| D052 | ISSUE_LIFECYCLE.md:530–534 | JSON objects/current status version/full Git ID; no branch/abbreviated commit evidence | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D053 | ISSUE_LIFECYCLE.md:538 | Triage classification enum and investigation/criteria summary | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D054 | ISSUE_LIFECYCLE.md:539 | Plan unique step IDs/files/risks/checks/assumptions and revision response contract | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D055 | ISSUE_LIFECYCLE.md:540 | Review identity, severity, source-backed blockers and reopening new-evidence contract | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D056 | ISSUE_LIFECYCLE.md:541–543 | Adjudication version/fingerprint/dispositions; actual approve scope; recorded worktree/branch adoption | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D057 | ISSUE_LIFECYCLE.md:544–545 | Implementation clean full commit; code review plan-step coverage/status/deviation justification/advisory checks | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D058 | ISSUE_LIFECYCLE.md:546–550 | Verify/ready/merge/repair/outcome input fields | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D059 | ISSUE_LIFECYCLE.md:552–554 | Severity enum; unresolved high/critical blocks; empty findings means actual review | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D060 | ISSUE_LIFECYCLE.md:556–561 | Unique check ID charset/48-character bound, argv/no shell, covers IDs, focused not full-suite commands | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D061 | ISSUE_LIFECYCLE.md:563–564 | Advisory preliminary-check fields and passed/failed/skipped enum | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs) |
| D062 | ISSUE_LIFECYCLE.md:589–604 | Source-specific plan and genuine review; implement names recorded absolute path/existing feature branch | [Design: evidence/input contracts](../../../../docs/ISSUE_LIFECYCLE.md#structured-evidence-and-operator-inputs), [Planning standards](planning.md) |
| D063 | ISSUE_LIFECYCLE.md:606–608 | Input objects do not establish approval; conversation authorization precedes recording | [Design: ownership](../../../../docs/ISSUE_LIFECYCLE.md#ownership) |
| D064 | ISSUE_LIFECYCLE.md:612–621 | Independent reusable package/methods/options; optional empty strings preserve defaults; standalone control optional | [Design: reusable extension](../../../../docs/ISSUE_LIFECYCLE.md#reusable-swamp-extension) |
| D065 | ISSUE_LIFECYCLE.md:620–624 | One task per behavior; repeated run refused; resume saved submission/known startup, not uncertain launch | [Design: reusable extension](../../../../docs/ISSUE_LIFECYCLE.md#reusable-swamp-extension) |
| D066 | ISSUE_LIFECYCLE.md:624–627 | Saved structured launch rejection; lifecycle explicit discard; standalone new identity/corrected inputs | [Recovery: rejected launches](recovery.md#rejected-launches) |
| D067 | ISSUE_LIFECYCLE.md:630–651 | Reusable workflow input schema and installed validate/run/resume/status interface in control repo | [Design: reusable extension](../../../../docs/ISSUE_LIFECYCLE.md#reusable-swamp-extension) |
| D068 | ISSUE_LIFECYCLE.md:654–658 | Correlated completed nonempty final/no-background result; state identity distinct from observation; answer not correctness | [Design: reusable extension](../../../../docs/ISSUE_LIFECYCLE.md#reusable-swamp-extension) |
| D069 | ISSUE_LIFECYCLE.md:665–671 | Ordered lifecycle checks, approved plan/clean review, exact clean branch/commit/own target outside merge lock | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification) |
| D070 | ISSUE_LIFECYCLE.md:672–680 | Exit/duration/output; dirty/moved/refused check invalidates later checks; other failures continue; child cleanup/hour timeout | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification) |
| D071 | ISSUE_LIFECYCLE.md:681–689 | Verification binds plan/branch/commit; check-log naming/type and actual retrieval query | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification) |
| D072 | ISSUE_LIFECYCLE.md:691–695 | Ready exact plan/commit; all invalidation causes; rollback-only-ready retains unchanged verification | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification) |
| D073 | ISSUE_LIFECYCLE.md:695–699 | Failed blocked checklist/log, no drive rerun, environment-only explicit verify, event history and exit 2/stderr/stdout | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification), [Recovery: review/verification](recovery.md#review-and-verification-failures) |
| D074 | ISSUE_LIFECYCLE.md:701–704 | Legacy no-check reviewer gate/disclosure; all new plans/revisions need checks | [Design: verification records](../../../../docs/ISSUE_LIFECYCLE.md#deterministic-verification) |
| D075 | ISSUE_LIFECYCLE.md:708–711 | Targeted development selection recorded with results/coverage; fix flakes here, rerun alone insufficient | [Design: development/merge gates](../../../../docs/ISSUE_LIFECYCLE.md#development-checks-and-the-merge-gate) |
| D076 | ISSUE_LIFECYCLE.md:713–718 | Full gate after rebase; Docker separate/relevant remote tests; isolated target/no target override/artifact copying | [Design: development/merge gates](../../../../docs/ISSUE_LIFECYCLE.md#development-checks-and-the-merge-gate) |
| D077 | ISSUE_LIFECYCLE.md:720–723 | Low-space managed-test temp settings, short private larger filesystem outside worktrees | [Recovery: review/verification](recovery.md#review-and-verification-failures) |
| D078 | ISSUE_LIFECYCLE.md:725–729 | Explicit scoped merge waiver only; cost/diff/prior pass/speed insufficient; skipped never passed | [Design: development/merge gates](../../../../docs/ISSUE_LIFECYCLE.md#development-checks-and-the-merge-gate) |
| D079 | ISSUE_LIFECYCLE.md:738–745 | Durable attempt JSON and complete stdout/stderr CI log paths | [Design: merge evidence](../../../../docs/ISSUE_LIFECYCLE.md#merge-gate-evidence) |
| D080 | ISSUE_LIFECYCLE.md:746–753 | Terminal/Swamp stream, phase markers/missing-phase semantics; bounded failure lists, truncation and partial null-exit log | [Design: merge evidence](../../../../docs/ISSUE_LIFECYCLE.md#merge-gate-evidence) |
| D081 | ISSUE_LIFECYCLE.md:755–759 | Prior failed receipts and durable mergeFailures across repair/rollback/eventual merge | [Design: merge evidence](../../../../docs/ISSUE_LIFECYCLE.md#merge-gate-evidence) |
| D082 | ISSUE_LIFECYCLE.md:761–766 | Failure presentation/history; repair summary/log in lastFailure to next implementer; retry preserves evidence | [Design: merge evidence](../../../../docs/ISSUE_LIFECYCLE.md#merge-gate-evidence) |
| D083 | ISSUE_LIFECYCLE.md:770–779 | Independent issue work, per-issue serialization, common-Git OS lock, current-main acquisition, non-FIFO, no manual bypass | [Design: integration boundary](../../../../docs/ISSUE_LIFECYCLE.md#parallel-development-and-serialized-integration) |
| D084 | ISSUE_LIFECYCLE.md:781–784 | Local-only operations; branch/worktree cleanup separate; retained failed worktrees | [Design: integration boundary](../../../../docs/ISSUE_LIFECYCLE.md#parallel-development-and-serialized-integration) |
| D085 | ISSUE_LIFECYCLE.md:788–793 | Inspect status/exact workflow via installed history/data help, not internal-file edits; state/events resources | [Recovery: inspect blocker](recovery.md#inspect-the-blocker) |
| D086 | ISSUE_LIFECYCLE.md:797–811 | Session/timeout/dialog/rejection/schema/partial-import/conflict/gate/interruption/landed-close-out routes | [Skill: blocker routing](../SKILL.md#blocker-routing) |
| D087 | ISSUE_LIFECYCLE.md:805–806 | Changed Agenda assessment; repair review blockers with fresh evidence | [Recovery: review/verification](recovery.md#review-and-verification-failures), [Recovery: inspect blocker](recovery.md#inspect-the-blocker) |
| D088 | ISSUE_LIFECYCLE.md:812 | Unmet issue remains open with remaining work recorded | [Recovery: close-out](recovery.md#close-out-failures) |
| D089 | ISSUE_LIFECYCLE.md:814–816 | No reset/discard to fake clean retry; dirty/conflicted/changed target requires inspection; empty/cancelled not verification | [Recovery: integration failures](recovery.md#integration-failures), [Recovery: review/verification](recovery.md#review-and-verification-failures) |
| D090 | ISSUE_LIFECYCLE.md:820–834 | Extension and focused Python test entry points, installed smoke prerequisites/disposable fixtures | [Design: validation boundaries](../../../../docs/ISSUE_LIFECYCLE.md#validation-boundaries) |
| D091 | ISSUE_LIFECYCLE.md:836–851 | Injected/fake adapter and Cargo coverage vs limited live Claude check; full gate only at authorized merge | [Design: validation boundaries](../../../../docs/ISSUE_LIFECYCLE.md#validation-boundaries) |
| D092 | ISSUE_LIFECYCLE.md:855–865 | Source design/primitive separation, Agenda/local receipt adaptation and no synthetic review/unmet completion | [Design: origin](../../../../docs/ISSUE_LIFECYCLE.md#design-origin) |

## Audit and limits

The source-span correction audit checked all 114 skill entries and all 92
supplementary guide entries directly against the pinned baseline blobs. Ranges
include every clause named by their row, including clauses split across lines;
comma-separated ranges identify nonadjacent source clauses. This source check
is distinct from checking the destination text or local links.

The implementation audit compared the source clauses above with their destination
text, including separate authorization boundaries, saved worker/control identity,
revision allowance, current review, delivery/discard cases, rollback persistence,
verification and retained failed-merge evidence. In particular, completed-result,
terminal-failure, structured-not-sent and resource-free-rejection discard gates
remain distinct; explicit restart/rollback does not inherit discard prerequisites.

All eight phases and all eight handoff decisions were checked against
`extensions/behaviors/_lib/lifecycle.ts` and
`extensions/behaviors/_lib/issue_driver.ts`. Pending work precedes phase dispatch;
the blocker table includes a free-text catch-all. Explicit restart routing does
not require eager recovery loading. Local-link and focused consumer checks are
separate from this semantic audit. Review must still verify preservation clause
by clause: link existence or paragraph coverage alone cannot prove it.
