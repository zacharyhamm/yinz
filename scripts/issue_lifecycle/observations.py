"""Read-only consistency diagnostics. These observations never authorize import."""
import hashlib
import json


def state_digest(state):
    """The controller's canonical state digest: sorted keys, compact JSON, SHA-256."""
    text = json.dumps(state, sort_keys=True, separators=(",", ":"), ensure_ascii=False)
    return hashlib.sha256(text.encode("utf-8", "surrogatepass")).hexdigest()


def compact_submission(submission):
    """A submission as status reports it: its lifecycle states replaced by digests.

    Assignments and receipts saved before compaction embed whole states; the
    evidence record itself is never rewritten.
    """
    if not isinstance(submission, dict):
        return submission
    result = dict(submission)
    for key, field in (("assignment", "before"), ("receipt", "next")):
        record = result.get(key)
        if isinstance(record, dict) and field in record:
            record = dict(record)
            record[f"{field}Digest"] = state_digest(record.pop(field))
            result[key] = record
    return result


def dispatched_unchanged(state, task):
    """Whether the lifecycle still equals the state the task was dispatched against."""
    if state is None:
        return False
    if task.get("beforeDigest"):
        return state_digest(state) == task["beforeDigest"]
    return json.dumps(state, sort_keys=True) == json.dumps(task.get("before"), sort_keys=True)


def _pane_background(child):
    """Whether the child or its latest response reports background work for its pane.

    A timed-out wait carries Yinz's background flag only on the agent row, not in
    the top-level fields the child copies into backgroundTasks.
    """
    if child.get("backgroundTasks") or child.get("phase") == "background":
        return True
    try:
        response = json.loads(child.get("rawResponse") or "null")
    except ValueError:
        return False
    if not isinstance(response, dict):
        return False
    rows = [response] + [agent for agent in response.get("agents") or []
                         if isinstance(agent, dict) and agent.get("pane_id") == child.get("paneId")]
    return any(row.get("background") or row.get("background_tasks") for row in rows)


def diagnose(name, state, task, child, submission, runs):
    diagnostics = []
    identity = {"taskId": (task or {}).get("id"),
                "behaviorName": (task or {}).get("behaviorName"),
                "submissionId": (child or {}).get("submissionId"),
                "conversationId": (child or {}).get("conversationId")}

    def report(code, message, action):
        diagnostics.append({"code": code, "message": message, "identity": identity,
                            "action": action})

    resume = "Run resume-agent for the original assignment, then drive; do not resend its prompt."
    if task and task.get("status") in ("running", "applying"):
        if not child:
            report("child_state_missing", "Original worker state is unavailable; completion is unproven.",
                   "Inspect the original launch and child state before resume-agent; do not launch a replacement.")
        elif child.get("config") != task.get("config") or child.get("promptDigest") != task.get("promptDigest"):
            report("worker_identity_mismatch", "Child configuration or prompt digest differs from the assignment.",
                   "Inspect the exact original child and assignment; do not import this result.")
        elif submission and submission.get("status") == "accepted" and (
            submission.get("assignment", {}).get("taskId") != task.get("id") or
            submission.get("assignment", {}).get("digest") != task.get("submission", {}).get("assignmentDigest")
        ):
            report("evidence_identity_mismatch", "Accepted evidence belongs to a different assignment.",
                   "Inspect the original evidence slot and task digest; do not import or resubmit this receipt.")
        elif _pane_background(child):
            action = "Observe the original conversation until correlated final idle completion; accepted evidence is insufficient."
            if submission and submission.get("status") == "accepted":
                action += (" If drive stops on a background stall for this accepted evidence, or the pane closed, the user may"
                           f" explicitly authorize accept-submission with taskId {task.get('id')} and their actual request.")
            report("background_work", "Original worker still has unresolved background work.", action)
        elif child.get("phase") == "completed" and child.get("outcome") == "completed":
            report("completion_unreconciled", "Child reports durable completion; lifecycle import still requires validation.", resume)
        elif submission and submission.get("status") == "accepted":
            report("evidence_waiting_for_completion", "Evidence is accepted but worker termination is not proven.", resume)
    for run in runs:
        if run.get("inputs", {}).get("issueName") != name:
            continue
        if run.get("status") != "running":
            continue
        # Lifecycle completion is a concrete contradiction, not an age heuristic.
        # It says nothing about whether an old controller process is still alive.
        if state and state.get("phase") == "done":
            diagnostics.append({"code": "workflow_running_after_issue_done", "runId": run.get("runId"),
                                "identity": identity,
                                "message": "Workflow remains recorded running although this issue is done; controller liveness is unknown.",
                                "action": f"Inspect swamp workflow history get {run.get('runId')} --json and controller run diagnostics; leave historical status unchanged."})
    return diagnostics


# Advisory task recovery. It mirrors the TypeScript discard predicates in
# extensions/behaviors/_lib/yinz_agent.ts and drive's worker verdicts in
# _lib/worker_recovery.ts; fixtures/discard_cases.json keeps the predicates aligned, and
# fixtures/recovery_cases.json keeps task_recovery aligned with classifyWorkerFailure.
# Every method it names re-verifies its own conditions.

PREPARATION_FAILURES = frozenset({"failed", "target_changed", "target_closed", "status_unavailable",
                                  "interrupted", "timed_out"})
IDENTITY_CODES = frozenset({"child_state_missing", "worker_identity_mismatch", "evidence_identity_mismatch"})
# drive finishes a pending rollback's saved intent before anything else.
PENDING_ROLLBACK_METHOD = "drive"
USER_REQUEST = "<the user's actual request>"
ADVISORY = "Computed from durable records; the named method re-verifies every condition."
BOUNDED = ("drive retries this automatically within a bound, re-observing the original submission "
           "without resending its prompt; if drive reports the retry budget exhausted, rerun drive "
           "(each run starts a new budget).")
_ABSENT = object()


def _quoted(record, key):
    value = record.get(key, _ABSENT) if isinstance(record, dict) else _ABSENT
    return "absent" if value is _ABSENT else json.dumps(value)


def _condition(name, met, satisfied, problem):
    return {"name": name, "met": bool(met), "detail": satisfied if met else problem}


def _fold_background(value):
    tasks = value.get("background_tasks")
    tasks = list(tasks) if isinstance(tasks, list) else []
    if value.get("background") and not tasks:
        tasks = ["background"]
    return {**value, "background_tasks": tasks}


def _response(raw):
    """The saved agent response, or None when it is not a valid one."""
    if not isinstance(raw, str):
        return None
    try:
        value = json.loads(raw)
    except ValueError:
        return None
    if not isinstance(value, dict) or not isinstance(value.get("outcome"), str) or not value["outcome"].strip():
        return None
    agents = value.get("agents", [])
    if not isinstance(agents, list) or not all(
        isinstance(agent, dict) and isinstance(agent.get("pane_id"), str) and isinstance(agent.get("session"), str)
        and isinstance(agent.get("agent"), dict) and isinstance(agent["agent"].get("kind"), str)
        and isinstance(agent["agent"].get("state"), str)
        for agent in agents
    ):
        return None
    return {**_fold_background(value), "agents": [_fold_background(agent) for agent in agents]}


def _selected_agent(child, response):
    """Mirrors selectedAgent: (row, None) or (None, problem)."""
    matches = [agent for agent in response["agents"] if agent["pane_id"] == child.get("paneId")]
    if len(matches) != 1:
        return None, "Yinz did not identify exactly the recorded agent pane"
    agent = matches[0]
    config = child.get("config") or {}
    if agent["session"] != config.get("session") or agent["agent"]["kind"] != config.get("kind"):
        return None, "Agent pane has a different session or provider"
    if child.get("conversationId") and (agent["agent"].get("session_id") or None) != child["conversationId"]:
        return None, "Agent conversation changed; do not submit or retrieve against a replacement"
    return agent, None


def _missing_child():
    return [_condition("child_state", False, "", "no durable child state")]


def rejected_launch_conditions(child):
    """Mirrors rejectedLaunchProblems: all met exactly when the launch is definitively rejected."""
    if not child:
        return _missing_child()
    receipt = child.get("launchReceipt")
    if child.get("launchId") and receipt and not child.get("submissionId"):
        resources = receipt.get("resources") or {}
        return [
            _condition("receipt_launch", receipt.get("launch_id") == child.get("launchId"),
                       "launch receipt matches the saved launch",
                       f"launch receipt id {_quoted(receipt, 'launch_id')} differs from the saved launch {_quoted(child, 'launchId')}"),
            _condition("receipt_server", receipt.get("server_instance") == child.get("launchServer"),
                       "launch receipt server matches the saved server",
                       f"launch receipt server {_quoted(receipt, 'server_instance')} differs from the saved server {_quoted(child, 'launchServer')}"),
            _condition("receipt_request", receipt.get("request_digest") == child.get("launchRequestDigest"),
                       "launch receipt request digest matches the saved request",
                       "launch receipt request digest differs from the saved request"),
            _condition("receipt_outcome", receipt.get("outcome") in ("rejected", "cancelled", "failed"),
                       f"launch receipt outcome is {_quoted(receipt, 'outcome')}",
                       f"launch receipt outcome is {_quoted(receipt, 'outcome')}, expected rejected, cancelled or failed"),
            _condition("receipt_process", receipt.get("process") in ("not_started", "exited"),
                       f"launch receipt process is {_quoted(receipt, 'process')}",
                       f"launch receipt process is {_quoted(receipt, 'process')}, expected not_started or exited"),
            _condition("cleanup_complete", not resources.get("cleanup_pending"),
                       "no launch cleanup is pending", "cleanup is still pending"),
            _condition("receipt_closed", receipt.get("disposition") == "closed",
                       "receipt disposition is closed",
                       f"receipt disposition is {_quoted(receipt, 'disposition')}, expected closed"),
        ]
    why = ("a submission exists, so the version-two launch receipt does not apply,"
           if child.get("launchId") and receipt else "no version-two launch receipt")
    retained = [label for present, label in (
        ("paneId" in child, "spawned pane"), ("tabId" in child, "tab"),
        ("conversationId" in child, "conversation"), ("submissionId" in child, "submission"),
        ("turnId" in child, "turn"), (bool(child.get("backgroundTasks")), "background tasks"),
    ) if present]
    conditions = [
        _condition("phase", child.get("phase") == "launch_rejected", "phase is launch_rejected",
                   f"{why} and phase is {_quoted(child, 'phase')}, not launch_rejected"),
        _condition("outcome", child.get("outcome") == "rejected", "outcome is rejected",
                   f"outcome is {_quoted(child, 'outcome')}, expected rejected"),
        _condition("no_retained_resources", not retained, "the saved state retains no resources",
                   f"saved rejection retains resources: {', '.join(retained)}"),
    ]
    if "rawResponse" not in child:
        return conditions + [_condition("saved_response", False, "", "no saved rejection response")]
    response = _response(child["rawResponse"])
    conditions.append(_condition("saved_response", response is not None, "a saved rejection response exists",
                                 "saved rejection response is not a valid agent response"))
    if response is None:
        return conditions
    retained = [label for present, label in (
        (response.get("spawned") is not None, "spawned pane"), (bool(response["agents"]), "agent rows"),
        (response.get("submission_id") is not None, "submission"), (bool(response.get("acknowledged")), "acknowledgement"),
        (response.get("turn_id") is not None, "turn"), (bool(response["background_tasks"]), "background tasks"),
        (response.get("project") is not None, "project"), (response.get("restarted") is not None, "restarted pane"),
        (response.get("launch_resources") is not None, "launch resources"),
    ) if present]
    return conditions + [
        _condition("response_outcome", response["outcome"] == "rejected", "response outcome is rejected",
                   f"response outcome is {json.dumps(response['outcome'])}, expected rejected"),
        _condition("response_no_resources", not retained, "the saved rejection retains no resources",
                   f"saved rejection retains resources: {', '.join(retained)}"),
    ]


def terminal_failure_conditions(child):
    """Mirrors terminalFailureProblems. This is the precheck: discard-agent also re-waits
    the submission and requires fresh idle status."""
    if not child:
        return _missing_child()
    conditions = [
        _condition("pane_id", child.get("paneId"), "a pane id is saved", "no pane id"),
        _condition("conversation_id", child.get("conversationId"), "a conversation id is saved", "no conversation id"),
        _condition("submission_id", child.get("submissionId"), "a submission id is saved", "no submission id"),
        _condition("phase", child.get("phase") in ("waiting", "submitting"), f"phase is {_quoted(child, 'phase')}",
                   f"phase is {_quoted(child, 'phase')}, expected waiting or submitting"),
    ]
    if not child.get("rawResponse"):
        return conditions + [_condition("saved_response", False, "", "no saved submission response")]
    response = _response(child["rawResponse"])
    conditions.append(_condition("saved_response", response is not None, "the latest response is valid",
                                 "latest response is not a valid agent response"))
    if response is None:
        return conditions
    agent, problem = _selected_agent(child, response)
    conditions += [
        _condition("selected_agent", agent is not None, "the latest response names the recorded agent",
                   f"latest response: {problem}"),
        _condition("submission_matches",
                   response.get("submission_id", _ABSENT) == child.get("submissionId", _ABSENT),
                   "latest response belongs to the saved submission",
                   "latest response submission differs from the saved submission"),
    ]
    if response.get("prompt_delivery") == "not_sent":
        background = response["background_tasks"] or (agent or {}).get("background_tasks") or child.get("backgroundTasks")
        return conditions + [
            _condition("preparation_failure", response["outcome"] in PREPARATION_FAILURES,
                       f"structured not_sent outcome {json.dumps(response['outcome'])} is a preparation failure",
                       f"structured not_sent outcome {json.dumps(response['outcome'])} is not a preparation failure"),
            _condition("not_acknowledged", not response.get("acknowledged"), "not_sent response is unacknowledged",
                       "not_sent response is acknowledged"),
            _condition("no_response_turn", response.get("turn_id") is None, "not_sent response has no turn id",
                       "not_sent response has a turn id"),
            _condition("no_saved_turn", "turnId" not in child, "saved state has no turn id", "saved state has a turn id"),
            _condition("no_background", not background, "no background tasks are running",
                       "background tasks are running"),
        ]
    outcome = response["outcome"]
    failed = outcome == "failed" and bool(response.get("acknowledged"))
    return conditions + [_condition(
        "acknowledged_failed", failed, "latest response is an acknowledged failed",
        f"latest response outcome {json.dumps(outcome)}{' (unacknowledged)' if outcome == 'failed' else ''} "
        "is neither an acknowledged failed nor a structured not_sent preparation failure")]


def completed_worker_conditions(child):
    """Mirrors completedWorkerProblems. It carries no fresh observation."""
    if not child:
        return _missing_child()
    background = child.get("backgroundTasks") or []
    return [
        _condition("phase", child.get("phase") in ("completed", "completed_unusable"),
                   f"phase is {_quoted(child, 'phase')}",
                   f"phase is {_quoted(child, 'phase')}, expected completed or completed_unusable"),
        _condition("outcome", child.get("outcome") == "completed", "outcome is completed",
                   f"outcome is {_quoted(child, 'outcome')}, expected completed"),
        _condition("no_background", not background, "no background tasks are running",
                   f"background tasks are running: {', '.join(background)}"),
        _condition("pane_id", child.get("paneId"), "a pane id is saved", "no pane id"),
        _condition("conversation_id", child.get("conversationId"), "a conversation id is saved", "no conversation id"),
        _condition("submission_id", child.get("submissionId"), "a submission id is saved", "no submission id"),
    ]


def _all_met(conditions):
    return all(condition["met"] for condition in conditions)


def _correlated_timeout(task, child):
    """Mirrors worker_recovery.correlatedTimeout without the error-freshness check,
    which only drive can make; drive re-checks it against the error it receives."""
    phase = child.get("phase")
    if (phase not in ("spawned", "submitting", "waiting", "background") or child.get("failureKind") != "timeout"
            or child.get("outcome") != "timed_out" or not child.get("paneId") or not child.get("rawResponse")
            or not child.get("lastError")
            or (phase != "spawned" and (not child.get("conversationId") or not child.get("submissionId")))):
        return False
    response = _response(child["rawResponse"])
    if response is None:
        return False
    matches = [agent for agent in response["agents"] if agent["pane_id"] == child["paneId"]]
    if len(matches) != 1:
        return False
    agent, config = matches[0], task.get("config") or {}
    if phase == "submitting" and not (response.get("acknowledged") and response.get("prompt_delivery") == "started"
                                      and response.get("submission_id") == child.get("submissionId")):
        return False
    return (response["outcome"] == "timed_out"
            and (not child.get("launchServer") or not response.get("server_instance")
                 or response["server_instance"] == child["launchServer"])
            and (not child.get("turnId") or not response.get("turn_id") or response["turn_id"] == child["turnId"])
            and (not agent.get("cwd") or agent["cwd"] == config.get("cwd"))
            and response.get("prompt_delivery") != "not_sent"
            and (phase in ("spawned", "background") or bool(response.get("acknowledged")) or bool(response.get("turn_id")))
            and (phase == "spawned" or response.get("submission_id", _ABSENT) == child.get("submissionId", _ABSENT)
                 or (phase == "background" and response.get("submission_id") is None))
            and agent["session"] == config.get("session") and agent["agent"]["kind"] == config.get("kind")
            and (child.get("conversationId") is None or agent["agent"].get("session_id") == child["conversationId"])
            and agent["agent"]["state"].lower() == "busy"
            and agent.get("not_ready_reason") in (None, "not_idle", "unresolved_turn", "unconsumed_input"))


def _latest_outcomes(child):
    response = _response(child.get("rawResponse"))
    outcomes = {child.get("outcome"), (response or {}).get("outcome")}
    error = child.get("lastError") or ""
    if ": " in error:
        outcomes.add(error.rsplit(": ", 1)[1].strip())
    return outcomes - {None}


def _agent_settings(task):
    config = task.get("config") or {}
    settings = {"role": task.get("role")}
    for key in ("session", "kind", "model", "effort", "permissionMode", "startupTimeoutSeconds", "timeoutSeconds"):
        value = config.get(key)
        if value is not None and value != "":
            settings[key] = value
    return settings


def task_recovery(state, task, child, submission, rollback, diagnostics):
    """Advisory recovery for the current pane task. Pure: no I/O and no mutation.
    The first matching rule wins."""
    task_id = (task or {}).get("id")
    accepted = bool(submission) and submission.get("status") == "accepted"
    restart = {"method": "restart-agent",
               "input": {"taskId": task_id, "request": USER_REQUEST, **_agent_settings(task or {})},
               "requiresUserRequest": True,
               "when": "the user directs a fresh replacement worker for the same assignment"}
    rollback_alternative = {"method": "rollback",
                            "input": {"step": "<the lifecycle step the user chose>", "request": USER_REQUEST},
                            "requiresUserRequest": True,
                            "when": "the user directs a return to an earlier lifecycle step"}
    unresolved = bool(task) and task.get("status") in ("running", "applying")
    discard = None
    if unresolved:
        conditions = {"rejectedLaunch": rejected_launch_conditions(child),
                      "terminalFailure": terminal_failure_conditions(child),
                      "completedWorker": completed_worker_conditions(child)}
        basis = next((name for key, name in (("rejectedLaunch", "rejected_launch"),
                                             ("terminalFailure", "terminal_failure"),
                                             ("completedWorker", "completed_unusable"))
                      if _all_met(conditions[key])), None)
        # discard-agent refuses once lifecycle fields changed since dispatch.
        unchanged = dispatched_unchanged(state, task)
        # Only a rejected launch is proven by durable records alone; the other bases
        # are verified by discard-agent's own fresh observation.
        eligible = False if not basis or not unchanged else True if basis == "rejected_launch" else "unverified"
        discard = {"eligible": eligible, "basis": basis, "lifecycleUnchanged": unchanged, "conditions": conditions}

    def result(kind, summary, command=None, requires=False, alternatives=(), last_error=None, **extra):
        return {"kind": kind, "taskId": task_id, "role": (task or {}).get("role"),
                "behaviorName": (task or {}).get("behaviorName"), "summary": summary,
                "command": command, "requiresUserRequest": requires, "alternatives": list(alternatives),
                "discard": discard, "lastError": last_error, "advisory": ADVISORY, **extra}

    def command(method, inputs=None):
        return {"method": method, "input": inputs or {}}

    if rollback and rollback.get("status") == "pending":
        return result(
            "finish_rollback",
            f"Rollback {rollback.get('id')} to {rollback.get('step')} is incomplete. {PENDING_ROLLBACK_METHOD} "
            "finishes its saved intent first; that intent already carries the user's request.",
            command(PENDING_ROLLBACK_METHOD),
            alternatives=[{"method": "resume-rollback", "input": {}, "requiresUserRequest": False,
                           "when": "finish the saved rollback without driving further"},
                          {**rollback_alternative, "when": "the user directs a different rollback, superseding this one"}])
    status = (task or {}).get("status")
    if not task or status in ("discarded", "superseded") or (status == "completed" and task.get("cleanup") != "pending"):
        return result("none", "No pane task needs recovery; follow the current handoff.")
    if task.get("discard"):
        return result("finish_discard",
                      "A discard intent is recorded but unfinished; drive completes it before anything else.",
                      command("drive"), last_error=task.get("lastError"))
    if status == "completed":
        refusal = (child or {}).get("cleanupRefusal")
        error = task.get("lastError") or ""
        prefix = "Worker cleanup blocked: "
        guard = (error[len(prefix):].split(": ", 1)[0] if error.startswith(prefix)
                 else (refusal or {}).get("outcome") or "unknown")
        matches = bool(refusal) and refusal.get("submissionId") == child.get("submissionId") \
            and refusal.get("conversationId") == child.get("conversationId")
        if refusal and refusal.get("retryable") and matches:
            how = f"The refusal is a transient guard on the same worker; {BOUNDED}"
        else:
            how = ("The refusal is not a transient guard on the same worker; inspect the pane and resolve the "
                   "guard deliberately, then rerun drive.")
        shown = None if not refusal else {key: refusal.get(key) for key in
                                          ("outcome", "message", "retryable", "thrown", "observedAt")}
        return result("retry_cleanup",
                      f"The result is imported but worker cleanup is blocked by the {guard} guard. {how} "
                      "A leftover unconsumed_input marker never blocks cleanup.",
                      command("drive"), last_error=task.get("lastError"),
                      cleanupRefusal=shown and {**shown, "matchesWorker": matches})
    identity = sorted({entry.get("code") for entry in diagnostics or ()} & IDENTITY_CODES)
    if identity or not child:
        return result("inspect",
                      f"Worker identity is unproven ({', '.join(identity) or 'child_state_missing'}); inspect the "
                      "original child, launch and evidence before anything else. No command applies automatically.",
                      alternatives=[restart, rollback_alternative], last_error=task.get("lastError"))
    continuation = task.get("continuation")
    if continuation and (child.get("continuation") or {}).get("id") != continuation.get("id"):
        return result("continuation_unsent",
                      "The recorded continuation has not reached the worker, and drive never sends its prompt. "
                      "Rerunning continue-agent with the user's same request delivers the saved continuation.",
                      command("continue-agent", {"taskId": task_id, "request": USER_REQUEST}), True,
                      alternatives=[restart, rollback_alternative], last_error=task.get("lastError"))
    phase, background = child.get("phase"), child.get("backgroundTasks") or []
    if phase == "completed" and child.get("outcome") == "completed" and not background and accepted:
        return result("resume",
                      f"The worker completed and its evidence is accepted (completion unreconciled). {BOUNDED}",
                      command("drive"), last_error=task.get("lastError"))
    # Until a failure is recorded, drive simply observes the background work. Once one is,
    # classifyWorkerFailure retries it only as a correlated timeout or a transport error (the
    # rules below); any other outcome falls through to the discard, interrupted and dialog rules.
    if (phase == "background" or background) and not child.get("lastError"):
        return result("resume",
                      "The worker has unresolved background work and no recorded failure; drive observes the same "
                      "conversation's final idle answer. If that wait times out on the original, busy target, "
                      f"{BOUNDED} Any other outcome stops drive for a deliberate decision.",
                      command("drive"), last_error=task.get("lastError"))
    if _correlated_timeout(task, child):
        return result("resume",
                      f"An ordinary {phase} wait timed out on the original, acknowledged target. {BOUNDED}",
                      command("drive"), last_error=task.get("lastError"))
    if (child.get("outcome") == "transport_error" and child.get("submissionId") and child.get("paneId")
            and phase in ("waiting", "submitting", "background") and child.get("lastError")):
        return result("resume", f"A transport error was recorded after submission. {BOUNDED}",
                      command("drive"), last_error=task.get("lastError"))
    if ((task.get("config") or {}).get("control") and phase == "preflight" and not child.get("launchId")
            and not child.get("paneId") and not child.get("submissionId")
            and child.get("outcome") == "preflight_transport" and child.get("lastError")):
        return result("resume",
                      "Launch preflight failed in transport before any launch intent; drive retries the preflight "
                      "within a bound and never spawns after a launch intent; if drive reports the retry budget "
                      "exhausted, rerun drive (each run starts a new budget).",
                      command("drive"), last_error=task.get("lastError"))
    discard_command = command("discard-agent", {"request": USER_REQUEST})
    if discard["basis"] == "rejected_launch" and discard["lifecycleUnchanged"]:
        return result("discard",
                      "The launch was definitively rejected and retains no resources. discard-agent with the user's "
                      "request records it; a new agent or drive with corrected configuration follows.",
                      discard_command, True, last_error=task.get("lastError"))
    if discard["basis"] == "terminal_failure" and discard["lifecycleUnchanged"]:
        return result("discard",
                      "The saved response proves a terminal provider failure or explicit non-delivery. discard-agent "
                      "with the user's request re-waits the original submission and requires fresh idle status.",
                      discard_command, True, alternatives=[restart, rollback_alternative],
                      last_error=task.get("lastError"))
    if discard["basis"] == "completed_unusable" and discard["lifecycleUnchanged"] and not accepted:
        discard_alternative = {**discard_command, "requiresUserRequest": True,
                               "when": "the user abandons this worker's result; discard-agent needs confirmedStopped "
                                       "only if it cannot observe the pane listed and idle (a pane missing from its "
                                       "session may have warped or outlived its server)"}
        if child.get("cleanedUp"):
            return result("continue_or_discard",
                          "The worker completed without accepted evidence, and its pane is already cleaned up, so "
                          "continue-agent cannot reach it. discard-agent with the user's request applies; an absent "
                          "pane is not stop evidence, so after confirming the stop pass confirmedStopped.",
                          discard_command, True, alternatives=[restart, rollback_alternative],
                          last_error=task.get("lastError"))
        return result("continue_or_discard",
                      "The worker completed without accepted evidence. continue-agent with the user's request asks the "
                      "same conversation to submit its existing findings; discard-agent is the alternative.",
                      command("continue-agent", {"taskId": task_id, "request": USER_REQUEST}), True,
                      alternatives=[discard_alternative, restart, rollback_alternative],
                      last_error=task.get("lastError"))
    outcomes = _latest_outcomes(child)
    if "interrupted" in outcomes:
        return result("interrupted",
                      "The latest outcome is interrupted. drive's resume uses wait --submission, which pane input cannot "
                      "end; if that reports interrupted, it is the turn's own terminal outcome. Neither continue-agent "
                      "(needs a completed worker) nor discard-agent (needs failed or not_sent) applies then, so only "
                      "restart-agent or rollback with the user's request does.",
                      command("drive"), alternatives=[restart, rollback_alternative],
                      last_error=task.get("lastError"))
    if "needs_input" in outcomes:
        return result("dialog",
                      "The worker is waiting on a dialog. Resolve it deliberately in the pane (Yinz agents skill), "
                      "then run drive; nothing answers dialogs automatically.",
                      alternatives=[{**command("drive"), "requiresUserRequest": False,
                                     "when": "after the dialog is resolved deliberately"},
                                    restart, rollback_alternative],
                      last_error=task.get("lastError"))
    changed = (" Lifecycle fields changed since the task was dispatched, so discard-agent would refuse; "
               "resume the task before discarding it." if discard["basis"] and not discard["lifecycleUnchanged"] else "")
    return result("inspect",
                  "No automatic or discard route applies (for example an uncertain launch, a malformed or transport "
                  "failure, or a changed target). Inspect the pane and records; discard.conditions shows why discard "
                  f"does not apply.{changed}",
                  alternatives=[restart, rollback_alternative], last_error=task.get("lastError"))
