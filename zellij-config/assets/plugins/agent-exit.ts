// Attach a project-owned Swamp workflow. Retain the source until Leave.
import z, { definePlugin, type Event } from "zellij:plugin";
function required<T>(value: T): NonNullable<T> {
  if (value == null) throw new Error("Missing required plugin state");
  return value;
}

import { view } from "zellij:workflow-view";
let acting = false,
  busy = false,
  status = "Agent exited.",
  workflow = false;
let directory: string | undefined, launchToken: string | undefined, pane: number | undefined;
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
async function launch(config: Record<string, string>): Promise<void> {
  const ownPane = { Plugin: z.getPluginIds().plugin_id };
  const visible = (): boolean => {
    try {
      const info = z.command("GetPaneInfo", ownPane);
      return !!info && !info.is_suppressed;
    } catch {
      return false;
    }
  };
  // Subscribe before checking: load can precede placement of this hook.
  const placed = z.waitForEvent("PaneUpdate", visible);
  if (visible()) {
    placed.catch(() => {
      // Cancellation is expected before the placement completes.
    });
    placed.cancel();
  } else {
    await placed;
  }
  const token = config["agent_exit_token"];
  if (!token) throw new Error("Missing host exit correlation token");
  view.watch(token);
  try {
    await z.rpc.call({
      pluginUrl: "zellij:swamp-bridge",
      name: "swamp",
      timeoutMs: 30000,
      payload: {
        method: "workflow_run",
        work: token,
        key: "launch",
        directory: required(directory),
        inputs: {
          sourcePane: pane ?? null,
          kind: config["agent_kind"] ?? "",
          firecracker: config["agent_firecracker_vm"] ?? "",
        },
      },
    });
  } catch (error) {
    view.status = String(error);
    // A failed reply can follow dispatch; keep reconciling.
  }
  view.attach({ directory: required(directory), launch: token });
  busy = false;
  z.requestRender();
}
export default definePlugin({
  // oxlint-disable-next-line eslint/max-lines-per-function -- This lifecycle handler keeps its ordered protocol transitions together.
  load(config) {
    launchToken = config["agent_exit_token"];
    pane = config["agent_pane_id"] == null ? undefined : Number(config["agent_pane_id"]);
    directory = config["agent_firecracker_worktree"] ?? config["agent_cwd"];
    z.subscribe(["Key"]);
    if (config["swamp_owner"] || config["agent_workflow_owner"]) {
      status = "Managed helper exited. Its owning operation receives the exit event.";
      return;
    }
    if (!directory || config["agent_target"] === "ssh") {
      status = "No local control checkout is available. Enter: restart. Esc: return to scrollback.";
      return;
    }
    try {
      const snapshot = z.projectConfig.read(directory);
      if (snapshot.error || !snapshot.config.workflow)
        // oxlint-disable-next-line typescript/prefer-nullish-coalescing -- An empty wire string needs the same display fallback as an absent value.
        throw new Error(snapshot.error || "No exit workflow configured.");
    } catch (error) {
      status = `${String(error)}\nUse Hocus /project-setup to configure .zellij/swamp-workflows.kdl.`;
      return;
    }
    view.active = true;
    const paneAction = (action: string, key: string): void => {
      if (acting || view.submitting) return;
      acting = true;
      void z.rpc
        .call({
          pluginUrl: "zellij:swamp-bridge",
          name: "swamp",
          payload: {
            method: "pane_action",
            work: required(launchToken),
            key,
            action,
            cancel_active: true,
          },
        })
        .catch((error: unknown) => {
          view.action = action;
          view.notice = String(error);
          view.noticeError = true;
          z.requestRender();
        })
        .finally(() => {
          acting = false;
        });
    };
    view.load({
      onRestart: () => {
        paneAction("restart", "restart");
      },
      onFork: () => {
        paneAction("fork", "fork-" + z.monotonicMs());
      },
    });
    view.status = "Starting workflow...";
    workflow = true;
    busy = true;
    void launch(config).catch((error: unknown) => {
      view.status = String(error);
      z.requestRender();
    });
  },
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
  update(event: Event) {
    if (workflow) {
      if (
        typeof event === "object" &&
        "Key" in event &&
        view.pending.length === 0 &&
        event.Key.bare_key === "Delete"
      ) {
        if (!busy && !view.active && !view.submitting && pane != null) {
          z.command("CloseTerminalPane", pane);
          z.command("CloseSelf");
        }
        return true;
      }
      return view.update(event);
    }
    if (typeof event !== "object" || !("Key" in event)) return false;
    if (event.Key.bare_key === "Esc") z.command("CloseSelf");
    else if (event.Key.bare_key === "Enter" && !busy) {
      if (pane != null) z.command("RerunCommandPane", pane);
      z.command("CloseSelf");
    } else if (event.Key.bare_key === "Delete" && !busy) {
      if (pane != null) z.command("CloseTerminalPane", pane);
      z.command("CloseSelf");
    }
    return true;
  },
  pipe: (message) => {
    view.pipe(message);
  },
  render(rows, cols) {
    if (workflow)
      return (
        view.render(Math.max(0, rows - 1), cols) +
        z.ui
          .text(
            z.text.truncate(
              busy || view.active || view.submitting
                ? "Enter: activate button. Restart/Fork: cancel run and continue. Cancel run: explicit cancellation."
                : "Enter: activate button  Delete: leave and close held pane",
              cols,
            ),
          )
          .dim()
          .render({ x: 0, y: Math.max(0, rows - 1), width: cols, height: 1 })
      );
    return [
      ...status.split("\n").filter(Boolean),
      `Directory: ${directory ?? "unknown"}`,
      "Enter: restart agent  Esc: scrollback  Delete: close held pane",
    ]
      .slice(0, rows)
      .map((line, y) =>
        z.ui
          .text(z.text.truncate(z.text.sanitize(line), cols))
          .render({ x: 0, y, width: cols, height: 1 }),
      )
      .join("");
  },
});
