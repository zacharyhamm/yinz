// Bind with: hooks { agent_exit "file:agent-commit.ts"; }
import z, { definePlugin } from "yinz:plugin";
let cwd = "",
  pane: number | undefined,
  status = "";
async function commit(): Promise<void> {
  if (status) return;
  try {
    for (const [label, command] of [
      ["staging...", ["git", "add", "-A"]],
      ["committing...", ["git", "commit", "-m", "work from an agent session"]],
    ] as const) {
      status = label;
      z.requestRender();
      // oxlint-disable-next-line eslint/no-await-in-loop -- These operations must complete in order before the next state transition.
      const result = await z.runCommand({ command: [...command], cwd, maxBytesReturned: 8192 });
      if (result.exitCode !== 0) throw new Error(z.decodeUtf8(result.stderr));
    }
    status = "committed.";
  } catch (error) {
    status = String(error);
  }
  z.requestRender();
}
z.on("Key", (key) => {
  if (key.bare_key === "Enter") {
    if (pane != null) z.command("RerunCommandPane", pane);
    z.command("CloseSelf");
  } else if (key.bare_key === "Esc") {
    if (pane != null) z.command("CloseTerminalPane", pane);
    z.command("CloseSelf");
  } else if (
    typeof key.bare_key === "object" &&
    "Char" in key.bare_key &&
    key.bare_key.Char === "c"
  )
    void commit().catch((error: unknown) => {
      z.log(String(error));
    });
});
export default definePlugin({
  load(config) {
    pane = config["agent_pane_id"] ? Number(config["agent_pane_id"]) : undefined;
    cwd = config["agent_cwd"] ?? "";
    z.requestPermission(["RunCommands", "ChangeApplicationState"]);
    z.subscribe(["Key"]);
  },
  render(rows, cols) {
    return (
      "\n".repeat(Math.max(Math.floor(rows / 2) - 2, 0)) +
      [
        "Agent exited. What next?",
        "",
        `Directory: ${cwd}`,
        "",
        status,
        "",
        "<ENTER> restart  <C> stage & commit  <ESC> close pane",
      ]
        .map((line) => z.text.center(line, cols))
        .join("\n")
    );
  },
});
