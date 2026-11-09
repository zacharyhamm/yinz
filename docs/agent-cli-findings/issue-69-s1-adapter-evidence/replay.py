#!/usr/bin/env python3
"""S1 diagnostic only. No Yinz/provider state, sockets, processes, or timers.

Native normalization is tested separately from synthetic reducer schedules.
An abstract disposition/answer event is NOT proof the provider exposes that input.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re


HERE = Path(__file__).resolve().parent
OLD = HERE.parent / "issue-69-notification-evidence"


def notice(text):
    # Structural envelope only. Do not search arbitrary answer prose for identifiers.
    if not isinstance(text, str) or not text.startswith("<task-notification>\n"):
        return None
    head = text.split("</task-notification>", 1)[0]
    fields = {}
    for tag in ("task-id", "tool-use-id", "status"):
        values = re.findall(r"(?:^|\n)<" + tag + r">([^<>\n]+)</" + tag + r">", head)
        if len(values) != 1:
            return None
        fields[tag] = values[0]
    return fields


def native_hook(row, scope):
    """Allowlisted fields needed BEFORE generic prose/response pruning."""
    if row.get("hook_event_name") != "PostToolUse" or row.get("is_error"):
        return None
    if not isinstance(row.get("session_id"), str) or row["session_id"].startswith("served:"):
        return None
    common = dict(scope=scope, owner=row.get("agent_id", "root"),
                  prompt=row.get("prompt_id"), tool=row.get("tool_use_id"))
    if not common["prompt"] or not common["tool"] or not row.get("session_id"):
        return None
    response = row.get("tool_response") or {}
    if not isinstance(response, dict):
        return None
    name = row.get("tool_name")
    task = None
    if name == "Bash":
        task = response.get("backgroundTaskId")
    elif name == "Agent" and response.get("status") == "async_launched" and response.get("isAsync") is True:
        task = response.get("agentId")
    if isinstance(task, str) and task:
        return dict(op="acquire", task=task, **common)
    if name == "TaskStop" and response.get("task_type") == "local_bash":
        requested = (row.get("tool_input") or {}).get("task_id")
        if requested and response.get("task_id") == requested:
            return dict(op="cancel", task=requested, **common)
    return None


class Model:
    def __init__(self):
        self.obligations = {}
        self.run_owner = {}
        self.completed = {}
        self.turns = set()
        self.registry = False
        self.available = True
        self.message = None

    def apply(self, e):
        op = e["op"]
        scope = e.get("scope", "proc1:conversation1")
        owner = e.get("owner", "root")
        if op == "acquire":
            key = (scope, owner, e["prompt"], e["tool"], e["task"])
            self.obligations.setdefault(key, dict(run=None, answer=None, done=False))
            if (scope, owner, e["prompt"]) not in self.completed:
                self.turns.add((scope, owner, e["prompt"]))
        elif op in ("root", "tool_result", "sidechain", "discard"):
            # These inputs must already be positively correlated. Anonymous removes
            # are intentionally absent here, even when their reason sounds terminal.
            candidates = [(k, v) for k, v in self.obligations.items()
                          if k[0] == scope and k[4] == e["task"] and k[3] == e["tool"]
                          and not v["done"] and v["run"] in (None, e["run"])]
            if len(candidates) != 1:
                return
            key, value = candidates[0]
            runkey = (scope, e["run"])
            if runkey in self.run_owner and self.run_owner[runkey] != key:
                return  # Old/duplicate run cannot settle a newer acquisition.
            self.run_owner[runkey] = key
            value["run"] = e["run"]
            if op == "discard":
                value["done"] = True
            else:
                value["answer"] = (scope, e.get("receiver", "root"), e["answer"])
                self.turns.add(value["answer"])
        elif op == "cancel":
            candidates = [(k, v) for k, v in self.obligations.items()
                          if k[0] == scope and k[4] == e["task"] and not v["done"]]
            # Restrict this no-notice shortcut to a unique acquired local shell
            # with no already-bound notice; production also needs native type guards.
            if len(candidates) == 1 and candidates[0][1]["run"] is None:
                candidates[0][1]["answer"] = (scope, owner, e["prompt"])
                self.turns.add((scope, owner, e["prompt"]))
        elif op == "end":
            key = (scope, owner, e["prompt"])
            self.completed[key] = e.get("text")
            self.turns.discard(key)
            self.message = e.get("text") or self.message
        elif op == "registry":
            self.registry = e["busy"]
        elif op == "unavailable":
            self.available = False
        elif op in ("marker_false", "empty_stop", "dequeue", "anonymous_remove", "raw_stop", "advance"):
            pass  # Neither a timer nor a negative sample proves a disposition.
        else:
            raise AssertionError("unknown fixture op: " + op)
        for value in self.obligations.values():
            if value["answer"] in self.completed:
                value["done"] = True
                self.turns.discard(value["answer"])

    def state(self):
        if not self.available:
            return "unavailable"  # Degradation is separate from successful settlement.
        return "pending" if self.registry or self.turns or any(
            not v["done"] for v in self.obligations.values()) else "idle"


def verify_sources(executable, private):
    data = executable.read_bytes()
    count = 0
    for index in (OLD / "provider-source-index.json", HERE / "provider-source-index.json"):
        doc = json.loads(index.read_text())
        assert hashlib.sha256(data).hexdigest() == doc["sha256"]
        for row in doc["entries"]:
            excerpt = data[row["offset"]:row["offset"] + row["length"]]
            assert hashlib.sha256(excerpt).hexdigest() == row["excerpt_sha256"], row["id"]
            if row["id"].startswith("A"):
                assert (private / (row["id"] + ".txt")).read_bytes() == excerpt
            count += 1
    return count


def measured():
    """Replay measured positive records; infer no provider order from arrival times."""
    rows = [json.loads(line) for line in (OLD / "interactive-capture.jsonl").read_text().splitlines()]
    models, ancestors, conversations, events, stop_text = {}, {}, {}, [], {}
    # The original selected projection omits intermediate attachment UUID edges.
    # Supplemental measured structural rows repair only those ancestry joins.
    # This lookup does not emit completion before the duration row is observed.
    boundaries = json.loads((HERE / "answer-boundaries.json").read_text())["boundaries"]
    for boundary in boundaries:
        for r in reversed(boundary["chain"]):
            prompt = r.get("promptId", ancestors.get((boundary["run"], r.get("parentUuid"))))
            assert prompt == boundary["prompt"]
            ancestors[boundary["run"], r["uuid"]] = prompt
    counts = dict(acquire=0, root=0, cancel=0, end=0)
    for line, row in enumerate(rows, 1):
        run, r = row["run"], row["record"]
        model = models.setdefault(run, Model())
        if row["kind"] == "hook" and r.get("hook_event_name") == "SessionStart":
            conversations[run] = r["session_id"]
        scope = run + ":" + conversations.get(run, "unknown")
        event = None
        if row["kind"] == "hook":
            if r.get("hook_event_name") == "Stop" and r.get("prompt_id"):
                stop_text[run, r["prompt_id"]] = r.get("last_assistant_message")
            event = native_hook(r, scope)
        elif row["kind"] == "registry" and "status" in r:
            event = dict(op="registry", busy=r["status"] in ("busy", "shell", "waiting"))
        elif row["kind"] == "transcript":
            parent = ancestors.get((run, r.get("parentUuid")))
            prompt = r.get("promptId", parent)
            if r.get("uuid") and prompt:
                ancestors[run, r["uuid"]] = prompt
            origin = r.get("origin") or {}
            if (r.get("type") == "user" and r.get("isSidechain") is False
                    and origin.get("kind") == "task-notification"
                    and origin.get("producer") == "session-task"):
                n = notice((r.get("message") or {}).get("content"))
                if n and origin.get("runId") and prompt:
                    event = dict(op="root", task=n["task-id"], tool=n["tool-use-id"],
                                 run=origin["runId"], answer=prompt, scope=scope)
            elif r.get("subtype") == "turn_duration" and prompt:
                event = dict(op="end", prompt=prompt, scope=scope,
                             text=stop_text.get((run, prompt)))
        if event:
            model.apply(event)
            if event["op"] in counts:
                counts[event["op"]] += 1
                events.append(dict(projection_line=line, source=row["source"], event=event))
    assert counts == dict(acquire=4, root=3, cancel=1, end=7), counts
    assert all(m.state() == "idle" for m in models.values())
    assert models["run2"].message == "ACK task-4\n\nFINAL_MIX"
    assert models["run3"].message == "CANCELLED_E"
    return dict(provenance="measured projection; modeled transitions, not live runtime", counts=counts, events=events)


def checks():
    fixtures = json.loads((HERE / "schedules.json").read_text())
    results = []
    for case in fixtures["cases"]:
        m = Model()
        for i, event in enumerate(case["events"]):
            m.apply(event)
            if "expect" in event:
                assert m.state() == event["expect"], (case["name"], i, m.state())
        if "message" in case:
            assert m.message == case["message"], case["name"]
        results.append(dict(name=case["name"], evidence=case["evidence"], result="pass"))
    # Literal producer schema from S5/S8/A16/A17: qCn/ll/Kr add no UUID.
    # Two different internal removals therefore serialize to the same observation.
    def removal(command, reason):
        r = dict(type="queue-operation", operation="remove", reason=reason)
        if reason != "dropped_by_hook":
            r["content"] = command["value"]
        for src, dst in [("uuid", "commandUuid"), ("deliveryId", "deliveryId")]:
            if src in command:
                r[dst] = command[src]
        return r
    a = dict(taskId="A", runId="RA", agentId="recipient-A", value="notice-A")
    b = dict(taskId="B", runId="RB", agentId="recipient-B", value="notice-B")
    assert removal(a, "dropped_by_hook") == removal(b, "dropped_by_hook")
    # Native parser rejects failed/foreground/partial launches. Task alias isn't
    # enabled just because its prose resembles an Agent result.
    base = dict(hook_event_name="PostToolUse", session_id="c", prompt_id="p",
                tool_use_id="t", tool_name="Agent", tool_response=dict(agentId="a", status="completed"))
    assert native_hook(base, "scope") is None
    assert native_hook(dict(base, is_error=True), "scope") is None
    assert native_hook(dict(base, tool_name="Task"), "scope") is None
    assert native_hook(dict(base, session_id="served:remote", tool_name="Bash",
                            tool_response=dict(backgroundTaskId="a")), "scope") is None
    assert notice("answer says <task-notification>\n<task-id>x</task-id>") is None
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path)
    parser.add_argument("--private", type=Path, default=Path("/tmp/issue-69-s1-adapter"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = dict(synthetic=checks(), measured=measured(),
                  anonymous_drop_projection="collision demonstrated; no task/run join")
    if args.executable:
        result["verified_source_excerpts"] = verify_sources(args.executable, args.private)
    if args.output:
        args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(dict(synthetic_cases=len(result["synthetic"]),
                          measured=result["measured"]["counts"],
                          source_excerpts=result.get("verified_source_excerpts"), result="pass")))


if __name__ == "__main__":
    main()
