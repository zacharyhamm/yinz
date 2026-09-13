// A command sequence in one pane, preserving each step's scrollback.
// This demonstrates pane commands; project automation belongs in a Swamp workflow.
import z, { definePlugin, type PaneSequence, type PaneStep } from "zellij:plugin";
let host = "",
  sequence: PaneSequence | undefined,
  skill = "deploy";
const lines = ["r: run the sequence, c: cancel it"];
function say(line: string): void {
  lines.push(line);
  z.requestRender();
}
async function run(): Promise<void> {
  if (sequence && ["running", "cancelling"].includes(sequence.status)) {
    say("already running");
    return;
  }
  const remote = `~/.hocus/skills/${skill}/SKILL.md`;
  const steps: PaneStep[] = host
    ? [
        { name: "list", path: "ssh", args: [host, "ls ~/.hocus/skills"], timeoutMs: 30000 },
        { name: "read", path: "ssh", args: [host, `cat ${remote}`], timeoutMs: 30000 },
        {
          name: "copy",
          path: "scp",
          args: [`${host}:${remote}`, `/tmp/${skill}.SKILL.md`],
          timeoutMs: 60000,
        },
      ]
    : [
        "echo listing; ls; sleep 1",
        "echo reading; head -3 AGENTS.md; sleep 1",
        "echo copying; sleep 1; echo done",
      ].map((script, i) => ({
        name: ["list", "read", "copy"][i] ?? "command",
        path: "sh",
        args: ["-c", script],
        timeoutMs: 30000,
      }));
  try {
    sequence = z.paneSequence({
      steps,
      floating: true,
      killGraceMs: 2000,
      onStep: (n, step, code) => {
        say(`step ${n} (${step.name}) exited ${code}`);
      },
    });
    const result = await sequence.done;
    say(
      `sequence ${result.status === "success" ? "completed" : result.status}; exit codes: ${JSON.stringify(result.exitCodes)}`,
    );
  } catch (error) {
    say(String(error));
  }
}
z.on("Key", (key) => {
  if (typeof key.bare_key === "object" && "Char" in key.bare_key) {
    if (key.bare_key.Char === "r")
      void run().catch((error: unknown) => {
        z.log(String(error));
      });
    else if (key.bare_key.Char === "c") {
      sequence?.cancel();
      say("cancelling...");
    }
  }
  return true;
});
export default definePlugin({
  defaults: { skill: "deploy" },
  load(config) {
    host = config["host"] ?? "";
    skill = config["skill"];
    if (!/^[\w.-]+$/u.test(skill) || host.startsWith("-")) throw new Error("Invalid host or skill");
    z.subscribe(["Key"]);
    z.requestPermission(["RunCommands", "ReadApplicationState", "ChangeApplicationState"]);
  },
  render: () => lines.join("\n"),
});
