"""Map one human decision's flags to method inputs, and chain mechanical steps.

Pure functions only: the launcher injects Swamp calls, so the chaining rules
are testable without a store. The launcher never fills a handoff id itself;
the user's words bind only to the handoff the controller presented.
"""

import re


UUID = re.compile(r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
FINDING = re.compile(r"^(review_code|review_ux)/(\S.*)$")
# The method input that carries --request for each command; drive maps it to revisionApproval.
REQUEST_FIELDS = {
    "approve": "request", "decide-review": "request", "approve-review-fixes": "request",
    "ready": "mergeRequest", "closeout": "request", "rollback": "request",
    "restart-agent": "request", "continue-agent": "request", "discard-agent": "request",
}
HANDOFF_COMMANDS = {"approve", "adjudicate-plan", "decide-review", "approve-review-fixes", "ready", "closeout", "drive"}
# Commands that continue with drive afterwards unless --no-drive.
CHAINED = {"approve", "adjudicate-plan", "decide-review", "approve-review-fixes", "ready",
           "implement", "repair", "refresh", "resume-rollback", "merge"}
# The only commands after which a merge can run; a CI waiver is accepted only
# for these. Other chained commands (for example decide-review on a legacy ready
# state, or resume-rollback to `merge`) can also reach an authorized integrate
# handoff, but stop there so a foreground decision never starts the CI gate.
MERGE_REACHING = {"ready", "drive", "merge"}


class InputError(ValueError):
    """A flag cannot be mapped to the command's inputs."""


def finding_ref(text):
    match = FINDING.fullmatch(text.strip())
    if not match:
        raise InputError(f"finding reference {text!r} must be review_code/<id> or review_ux/<id>")
    return {"role": match.group(1), "findingId": match.group(2)}


def _text(flag, value):
    if not value.strip():
        raise InputError(f"{flag} must not be empty")
    return value


def build_inputs(command, file_inputs, *, handoff=None, request=None, repair=(), ignore=(),
                 ignore_remaining=False, decision=None, skip_ci_request=None,
                 auto_repair=False, no_auto_repair=False):
    """Return the method inputs: the input file's object plus the mapped flags."""
    inputs = dict(file_inputs)

    def put(flag, key, value):
        if key in file_inputs:
            raise InputError(f"{flag} conflicts with {key} in the input file")
        inputs[key] = value

    if handoff is not None:
        if command not in HANDOFF_COMMANDS:
            raise InputError(f"--handoff applies only to {', '.join(sorted(HANDOFF_COMMANDS))}")
        if not UUID.fullmatch(handoff):
            raise InputError(f"--handoff {handoff!r} is not a handoff UUID")
    if request is not None:
        request = _text("--request", request)
        if command not in REQUEST_FIELDS and command != "drive":
            raise InputError(f"--request applies only to {', '.join(sorted({*REQUEST_FIELDS, 'drive'}))}")
    if (auto_repair or no_auto_repair) and command != "drive":
        raise InputError("--auto-repair and --no-auto-repair apply only to drive")
    if auto_repair and no_auto_repair:
        raise InputError("--auto-repair and --no-auto-repair conflict")
    if command == "drive":
        if auto_repair:
            # The user's standing instruction: repair every blocking finding of
            # this plan and rollback without asking.
            if handoff is not None or request is None:
                raise InputError("--auto-repair takes --request (the user's words) and no --handoff")
            put("--auto-repair", "autoRepair", {"request": request})
        elif no_auto_repair:
            if handoff is not None or request is not None:
                raise InputError("--no-auto-repair takes no --handoff or --request")
            put("--no-auto-repair", "autoRepair", {"withdraw": True})
        elif (handoff is None) != (request is None):
            raise InputError("drive takes --handoff and --request together, as one revision authorization")
        elif handoff is not None:
            put("--handoff/--request", "revisionApproval", {"handoffId": handoff, "request": request})
    else:
        if handoff is not None:
            put("--handoff", "handoffId", handoff)
        if request is not None:
            put("--request", REQUEST_FIELDS[command], request)
    if (repair or ignore or ignore_remaining) and command != "decide-review":
        raise InputError("--repair, --ignore and --ignore-remaining apply only to decide-review")
    if repair:
        put("--repair", "repair", [finding_ref(ref) for ref in repair])
    if ignore:
        put("--ignore", "ignore", [finding_ref(ref) for ref in ignore])
    if ignore_remaining:
        put("--ignore-remaining", "ignoreRemaining", True)
    if decision is not None:
        if command != "closeout":
            raise InputError("--decision applies only to closeout")
        put("--decision", "decision", decision)
    if skip_ci_request is not None:
        skip_ci_request = _text("--skip-ci-request", skip_ci_request)
        if command not in MERGE_REACHING:
            raise InputError(f"--skip-ci-request applies only to {', '.join(sorted(MERGE_REACHING))}")
        if command == "merge":
            put("--skip-ci-request", "skipCiRequest", skip_ci_request)
    return inputs


def check_chain_flags(command, *, no_drive=False, no_merge=False):
    if no_drive and command not in CHAINED:
        raise InputError(f"--no-drive applies only to {', '.join(sorted(CHAINED))}")
    if no_merge and command not in MERGE_REACHING - {"merge"}:
        raise InputError("--no-merge applies only to ready and drive, the commands that merge automatically")


def auto_merge_allowed(command, *, no_merge=False):
    """Whether the chain after this command may merge an authorized integrate handoff.

    merge itself already ran one merge, so its follow-up drive never merges again.
    """
    return not no_merge and command in MERGE_REACHING - {"merge"}


def should_auto_merge(handoff):
    """Merge only a fresh, explicitly authorized integrate handoff; never re-run an attempt.

    lastFailure is deliberately not checked: repair leaves it set until the next
    merge begins, and drive yields integrate only without an active merge failure.
    """
    # Compact handoffs carry a lifecycle summary; older ones embed the whole state.
    summary = handoff.get("lifecycle")
    if summary is None:
        state = handoff.get("state") or {}
        summary = {"phase": state.get("phase"), "mergeAuthorization": state.get("mergeAuthorization"),
                   "attemptId": (state.get("attempt") or {}).get("id")}
    return (handoff.get("status") == "current" and handoff.get("decision") == "integrate"
            and summary.get("phase") == "ready" and bool(summary.get("mergeAuthorization"))
            and not summary.get("attemptId"))


def continue_to_gate(drive, merge, first, later, *, allow_merge=True, merge_ran=False):
    """drive(inputs) -> handoff dict; merge() -> exit code. At most one merge per invocation.

    Returns the final handoff and the merge's exit code (0 when no merge ran).
    """
    handoff = drive(first)
    code = 0
    if allow_merge and not merge_ran and should_auto_merge(handoff):
        code = merge()
        handoff = drive(later)
    return handoff, code
