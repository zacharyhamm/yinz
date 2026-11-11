#!/usr/bin/env python3
"""Read exported lifecycle records only. Never connects to Swamp, Git or Agenda.

Normalize a complete `swamp data query --select` export with --normalize FILE.
Compare frozen normalized exports with --baseline FILE --pilot FILE. Output is
JSON on stdout; callers choose whether and where to retain it.
"""
from __future__ import annotations

import argparse
import copy
from collections import defaultdict
from datetime import datetime, timezone
import gzip
import hashlib
import json
from pathlib import Path
import statistics

TIMING_LIMITS = [
    "Elapsed time is the observed assignment-to-completion wall-clock span, including human waits, recovery and gate time; it is not active worker time.",
    "Missing/garbage-collected events and tasks can shorten the span and undercount rounds. No historical findings, causes or executions are inferred.",
    "No recorded post-completion regression is not evidence that no regression occurred.",
]


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def instant(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def unique(items, key):
    """Retain the latest exported projection of one assignment/operation."""
    result = {}
    for item in items:
        result[key(item)] = item
    return list(result.values())


def normalize(export):
    if export.get("limited") or export.get("nextCursor"):
        raise ValueError("Export is incomplete; fetch all pages before freezing a sample")
    grouped = defaultdict(list)
    for record in export["results"]:
        grouped[record["modelName"]].append(record)
    issues = []
    for name, records in sorted(grouped.items()):
        records.sort(key=lambda r: (r["createdAt"], r.get("version", 0)))
        states = [r for r in records if r["name"] == "state"]
        if not states or states[-1]["content"].get("phase") != "done":
            continue
        state = states[-1]["content"]
        completed = next((r["createdAt"] for r in states if r["content"].get("phase") == "done" and r["content"].get("receipt") == state.get("receipt")), states[-1]["createdAt"])
        reserved = []
        for record in records:
            if record["specName"] != "driver":
                continue
            for kind in ("automaticRevision", "revisionAuthorization"):
                task = (record["content"].get(kind) or {}).get("assignment")
                if task and task.get("contractVersion") == 2 and task.get("round"):
                    reserved.append(task)
        tasks = unique(reserved + [r["content"] for r in records if r["specName"] == "agent-task"], lambda t: t["id"])
        rounds = next((r["content"]["records"] for r in reversed(records) if r["name"] == "rounds"), [])
        recorded = {r["assignmentId"] for r in rounds}
        rounds = rounds + [t["round"] for t in tasks if t.get("round") and t["id"] not in recorded]
        # Preserve review evidence and exact identity while omitting prompts/issue prose.
        assignments = [{k: t[k] for k in ("id", "role", "createdAt", "contractVersion", "evidence") if k in t} | {
            "issueDigest": t["before"]["issue"]["digest"],
            "rollbackId": t["before"].get("rollbackId"),
        } | ({
            "completedRepairDecisionIds": [d["id"] for d in t["before"].get("reviewDecisions", []) if d.get("result")],
        } if t["role"] in ("review_code", "review_ux") else {}) for t in tasks]
        for task in assignments:
            if task.get("evidence"):
                task["evidence"] = copy.deepcopy({k: task["evidence"][k] for k in ("acceptedAt", "payload", "payloadDigest")})
                payload = task["evidence"]["payload"]
                payload["evidence"] = {k: payload["evidence"][k] for k in ("commit", "priorFindings", "responses", "advisoryEvidence") if k in payload["evidence"]}
        issues.append({
            "key": name, "completedAt": completed,
            "issueDigest": state["issue"]["digest"],
            "assignments": assignments, "rounds": rounds,
            "recoveries": unique([r["content"]["id"] for r in records if r["specName"] == "recovery"], lambda x: x),
            "requirementsDigests": sorted({r["content"]["issue"]["digest"] for r in states}),
            "gateFailures": unique([f for r in states for f in r["content"].get("mergeFailures", [])], lambda f: f["attemptId"]),
            "verificationFailures": sorted({r["content"]["verification"]["id"] for r in states if r["content"].get("verification", {}).get("status") == "failed"}),
            "repairDecisions": [{k: d[k] for k in ("id", "reviewFingerprint", "repair", "ignore") if k in d} | {"result": {"commit": d["result"]["commit"], "responses": d["result"].get("responses", [])} if d.get("result") else None} for d in state.get("reviewDecisions", [])],
            "observations": [],
            "sourceRefs": [{k: r[k] for k in ("modelName", "name", "version", "createdAt")} | {"projectionDigest": digest(r["content"])} for r in records],
        })
    return {"schemaVersion": 1, "frozenAt": datetime.now(timezone.utc).isoformat(), "sourceDigest": digest(export), "timingLimitations": TIMING_LIMITS, "issues": issues}


def effective_launch_causes(record):
    causes = list(record["launchCauses"])
    for correction in record.get("corrections", []):
        index = correction.get("replaces")
        # Corrections without a target are observations, not replacements.
        if isinstance(index, int) and 0 <= index < len(causes):
            causes[index] = correction["cause"]
    return causes


def first_repair_reviews(reviews, decision):
    """Use snapshot correlation: commits alone cannot prove a review is fresh."""
    eligible = sorted((t for t in reviews
                       if decision["id"] in t.get("completedRepairDecisionIds", [])
                       and t["evidence"]["payload"]["evidence"].get("commit") == decision["result"]["commit"]),
                      key=lambda t: t["createdAt"])
    first = {}
    for task in eligible:
        first.setdefault(task["role"], task)
    return first.values()


def issue_metrics(issue):
    tasks = unique(issue["assignments"], lambda t: t["id"])
    rounds = unique(issue.get("rounds", []), lambda r: r["assignmentId"])
    starts = [t["createdAt"] for t in tasks]
    elapsed = max(0, (instant(issue["completedAt"]) - min(map(instant, starts))).total_seconds() * 1000) if starts else None
    observations = issue.get("observations", [])
    for observation in observations:
        if not observation.get("evidence") or not observation.get("source"):
            raise ValueError("Quality observations require evidence and attribution")
    def observed(kind):
        values = [o for o in observations if o["kind"] == kind]
        return sum(o.get("count", 1) for o in values) if values else None
    unknown = sum(any(c["kind"] == "unknown" for c in effective_launch_causes(r)) for r in rounds)
    known_rounds = {r["assignmentId"] for r in rounds}
    unknown += sum(t["id"] not in known_rounds for t in tasks)
    reviews = [t for t in tasks if t["role"] in ("review_code", "review_ux") and t.get("evidence")]
    selected = [d for d in issue.get("repairDecisions", []) if d.get("repair") and d.get("result")]
    first_closure = None
    incomplete = set()
    if selected:
        decision = selected[0]
        expected = {(f["role"], f["findingId"]) for f in decision["repair"]}
        # The first submitted fresh review for each role after this repair is
        # authoritative for first-repair closure. Later fixes cannot erase it.
        outcomes = {}
        for task in first_repair_reviews(reviews, decision):
            for outcome in task["evidence"]["payload"]["evidence"].get("priorFindings", []):
                ref = outcome["finding"]
                key = (ref["role"], ref["findingId"])
                if key in expected:
                    outcomes[key] = outcome["outcome"]
        if expected <= outcomes.keys():
            first_closure = all(outcomes[k] == "verified_fixed" for k in expected)
    incomplete_repairs = set()
    for decision in selected:
        expected = {(f["role"], f["findingId"]) for f in decision["repair"]}
        if any(r["status"] in ("disputed", "unresolved") for r in decision["result"].get("responses", [])):
            incomplete_repairs.add(decision["id"])
        for task in first_repair_reviews(reviews, decision):
            evidence = task["evidence"]["payload"]["evidence"]
            if any(o["outcome"] == "persists" and (o["finding"]["role"], o["finding"]["findingId"]) in expected for o in evidence.get("priorFindings", [])):
                incomplete_repairs.add(decision["id"])
    reused = set()
    for task in tasks:
        evidence = task.get("evidence", {}).get("payload", {}).get("evidence", {})
        for response in evidence.get("responses", []):
            if response["status"] in ("unresolved", "disputed"):
                incomplete.add((task["id"], digest(response["finding"])))
        for advisory in evidence.get("advisoryEvidence", []):
            if advisory.get("reuse"):
                reused.add((task["id"], advisory["producingTask"], advisory["outputRef"]))
    for record in rounds:
        for correction in record.get("corrections", []):
            if correction["cause"]["kind"] == "incomplete_repair":
                incomplete.add((record["assignmentId"], correction["source"]))
    strata = {
        "requirementsChanged": len(issue.get("requirementsDigests", [issue["issueDigest"]])) > 1,
        "recovery": bool(issue.get("recoveries") or any(t.get("rollbackId") for t in tasks)),
        "gateFailure": bool(issue.get("gateFailures") or issue.get("verificationFailures")),
    }
    return {"key": issue["key"], "strata": strata, "rounds": len({t["id"] for t in tasks} | known_rounds), "elapsedMs": elapsed,
            "firstRepairClosure": first_closure, "incompleteRepairs": len(incomplete_repairs) if rounds else None, "incompleteRepairEvidence": len(incomplete) if rounds else None,
            "unsupportedReopening": observed("unsupported_reopening"), "requirementCoverage": observed("requirement_coverage"),
            "advisoryExecutionsAvoidedClaimed": len(reused) if rounds else None,
            "unknownCauseRounds": unknown, "laterGateDefects": observed("later_gate_defect"),
            "gateFailures": len(issue.get("gateFailures", [])) + len(issue.get("verificationFailures", [])),
            "postCompletionRegressions": observed("post_completion_regression")}


def cohort(issues):
    rows = [issue_metrics(i) for i in issues]
    groups = defaultdict(list)
    for row in rows:
        groups[json.dumps(row["strata"], sort_keys=True)].append(row)
    def summarize(items):
        times = [r["elapsedMs"] for r in items if r["elapsedMs"] is not None]
        closures = [r["firstRepairClosure"] for r in items if r["firstRepairClosure"] is not None]
        return {"issues": len(items), "rounds": sum(r["rounds"] for r in items), "medianRounds": statistics.median([r["rounds"] for r in items]) if items else None,
                "medianElapsedMs": statistics.median(times) if times else None, "firstRepairClosure": {"closed": sum(closures), "assessable": len(closures)},
                "unknownCauseRounds": sum(r["unknownCauseRounds"] for r in items)}
    return {"summary": summarize(rows), "strata": [{"stratum": json.loads(k), **summarize(v)} for k, v in sorted(groups.items())], "issues": rows}


def compare(baseline, pilot):
    if baseline["schemaVersion"] != 1 or pilot["schemaVersion"] != 1:
        raise ValueError("Unknown normalized export schema")
    if any(t.get("contractVersion") == 2 for i in baseline["issues"] for t in i["assignments"]):
        raise ValueError("Frozen pre-change sample contains new-contract assignments")
    candidates = [i for i in pilot["issues"] if instant(i["completedAt"]) > instant(baseline["frozenAt"]) and any(t.get("contractVersion") == 2 for t in i["assignments"])]
    candidates = sorted(unique(candidates, lambda i: i["key"]), key=lambda i: (i["completedAt"], i["key"]))[:20]
    if {i["key"] for i in baseline["issues"]} & {i["key"] for i in candidates}:
        raise ValueError("Pilot overlaps frozen baseline issues")
    return {"baselineDigest": digest(baseline), "pilotDigest": digest(pilot), "pilotTarget": 20,
            "baseline": cohort(baseline["issues"]), "pilot": cohort(candidates),
            "timingLimitations": list(dict.fromkeys(TIMING_LIMITS + baseline.get("timingLimitations", []) + pilot.get("timingLimitations", []))),
            "assessment": "No improvement claim. Compare matched scope and strata, repeated investigation/repair work and observed detection/safeguard compliance; inspect every quality regression. Small samples, missing attribution and absent follow-up limit conclusions."}


def read_export(path):
    data = gzip.decompress(path.read_bytes()) if path.suffix == ".gz" else path.read_bytes()
    return json.loads(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--normalize", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--pilot", type=Path)
    args = parser.parse_args()
    if args.normalize:
        result = normalize(read_export(args.normalize))
    elif args.baseline and args.pilot:
        result = compare(read_export(args.baseline), read_export(args.pilot))
    else:
        parser.error("Use --normalize FILE or --baseline FILE --pilot FILE")
    print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
