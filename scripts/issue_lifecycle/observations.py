"""Read-only consistency diagnostics. These observations never authorize import."""


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
        elif child.get("backgroundTasks") or child.get("phase") == "background":
            report("background_work", "Original worker still has unresolved background work.",
                   "Observe the original conversation until correlated final idle completion; accepted evidence is insufficient.")
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
