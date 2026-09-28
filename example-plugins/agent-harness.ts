// Start an agent, check its work, and return failures for a bounded number of retries.
import z, { definePlugin } from "yinz:plugin";
const lines: string[] = [];
let status = "starting";
const tail = (text: string, n: number): string =>
  text.split("\n").filter(Boolean).slice(-n).join("\n");
function say(line: string): void {
  lines.push(line);
  z.log(line);
  z.requestRender();
}
// oxlint-disable-next-line eslint/no-shadow, typescript/prefer-readonly-parameter-types -- The nested callback has its own independent operation state. This helper shares mutable plugin state and SDK wire types with its callers.
function record(config: { prompt: string; check: string }, status: string, attempts: number): void {
  z.data.define("run", {
    prompt: { kind: "Text", required: true },
    check: { kind: "Text", required: true },
    status: { kind: "Text", required: true, indexed: true },
    attempts: { kind: "Int", required: true },
  });
  z.data.apply([
    z.data.put("run", { prompt: config["prompt"], check: config["check"], status, attempts }),
  ]);
}
export default definePlugin({
  defaults: { prompt: "make the tests pass", check: "cargo x test", agent: "claude", retries: "3" },
  // oxlint-disable-next-line eslint/max-lines-per-function -- This lifecycle handler keeps its ordered protocol transitions together.
  async load(config) {
    try {
      await z.waitForPermissions([
        "RunCommands",
        "ReadApplicationState",
        "WriteToStdin",
        "ChangeApplicationState",
        "PluginData",
      ]);
      if (
        config["agent"] !== "claude" &&
        config["agent"] !== "codex" &&
        config["agent"] !== "cursor"
      )
        throw new Error("Unsupported agent");
      const pane = z.agent.spawn({
        kind: config["agent"],
        prompt: config["prompt"],
        branch: config["branch"] ?? null,
        allow_outside_worktree: !config["branch"],
        name: `harness: ${config["agent"]}`,
      });
      if (!("Terminal" in pane)) throw new Error("Expected terminal");
      say(`spawned ${config["agent"]} in pane ${pane.Terminal}: ${config["prompt"]}`);
      const turn = await z.agent.waitForTurn(pane, { fallback: "idle" });
      say("agent: " + tail(turn.message ?? "(no message)", 3));
      const cwd =
        z.agent.info(pane)?.cwd ?? (config["branch"] ? `.worktrees/${config["branch"]}` : "/host");
      const retries = Number(config["retries"]) || 3;
      for (let attempt = 1; attempt <= retries + 1; attempt++) {
        status = "checking";
        say(`check ${attempt}: ${config["check"]}`);
        // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
        const result = await z.runCommand({
          command: ["sh", "-c", config["check"]],
          cwd,
          maxBytesReturned: 65536,
        });
        if (result.exitCode === 0) {
          status = "green";
          say(status);
          record(config, status, attempt);
          z.command("Notify", [
            `harness: ${config["check"]} is green after ${attempt} attempt(s)`,
            null,
            null,
          ]);
          return;
        }
        if (attempt > retries) break;
        status = "red";
        say(`red (exit ${result.exitCode}); handing it back, ${attempt} of ${retries}`);
        // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
        const reply = await z.agent.prompt(
          pane,
          `The check \`${config["check"]}\` failed with exit ${result.exitCode}:\n${tail(z.decodeUtf8(result.stderr) + "\n" + z.decodeUtf8(result.stdout), 40)}\nFix it, then say what you changed.`,
          { fallback: "idle" },
        );
        say("agent: " + tail(reply.message ?? "(no message)", 3));
      }
      status = "gave up";
      say(`still red after ${retries} retries`);
      record(config, "red", retries);
      z.command("Notify", [
        `harness: ${config["check"]} is still red after ${retries} retries`,
        null,
        null,
      ]);
    } catch (error) {
      status = "failed";
      say(String(error));
    }
  },
  render(rows, cols) {
    return [
      z.ui.text(`harness: ${status}`).selected().render(),
      ...lines.slice(-Math.max(0, rows - 1)).map((line) => z.text.truncate(line, cols)),
    ].join("\n");
  },
});
