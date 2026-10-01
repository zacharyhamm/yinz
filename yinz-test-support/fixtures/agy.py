"""Controlled AGY process shared by local reporter and real SSH tests."""
import http.server
import json
import os
import subprocess
import sys
import threading

transcript, reporter = sys.argv[1], sys.argv[2:]
steps = [{"type": "CORTEX_STEP_TYPE_USER_INPUT", "status": "CORTEX_STEP_STATUS_DONE"}]
response = {"status": "CASCADE_RUN_STATUS_RUNNING", "numTotalSteps": 1,
            "trajectory": {"cascadeId": "fixture-conversation", "steps": steps}}


class API(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        assert self.path == "/exa.language_server_pb.LanguageServerService/GetCascadeTrajectory"
        assert self.headers["x-codeium-csrf-token"] == "fixture-token"
        assert json.loads(self.rfile.read(int(self.headers["Content-Length"]))) == {"cascadeId": "fixture-conversation"}
        body = json.dumps(response).encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), API)
threading.Thread(target=server.serve_forever, daemon=True).start()
os.environ["ANTIGRAVITY_LS_ADDRESS"] = "127.0.0.1:" + str(server.server_port)
os.environ["ANTIGRAVITY_CSRF_TOKEN"] = "fixture-token"


def record(kind, text):
    with open(transcript, "a") as f:
        f.write(json.dumps({"step_index": record.index, "type": kind, "status": "DONE", "content": text}) + "\n")
    record.index += 1


record.index = 0
record("USER_INPUT", "<USER_REQUEST>\nreview\n</USER_REQUEST>")


def report(event, **extra):
    payload = dict(hook_event_name=event, conversationId="fixture-conversation", transcriptPath=transcript, **extra)
    subprocess.run(reporter, input=json.dumps(payload).encode(), check=True)


def gate(name):
    print(name, flush=True)
    assert sys.stdin.readline()


report("PreInvocation", invocationNum=0, initialNumSteps=1)
record("PLANNER_RESPONSE", "FIRST")
report("Stop", fullyIdle=True)
gate("first-stop")
report("PreInvocation", invocationNum=1, initialNumSteps=2)
record("PLANNER_RESPONSE", "SECOND")
report("Stop", fullyIdle=True)
gate("continued-stop")
steps = steps + [{"type": "CORTEX_STEP_TYPE_PLANNER_RESPONSE", "status": "CORTEX_STEP_STATUS_DONE"}] * 2
response = {"status": "CASCADE_RUN_STATUS_IDLE", "numTotalSteps": len(steps),
            "trajectory": {"cascadeId": "fixture-conversation", "steps": steps}}
gate("api-idle")
server.shutdown()
