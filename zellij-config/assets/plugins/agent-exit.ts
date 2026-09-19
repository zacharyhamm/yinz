// Attach a project-owned Swamp workflow. Retain the source until Leave.
import z, { definePlugin, type Event } from "zellij:plugin";
function required<T>(value: T): NonNullable<T> {
  if (value == null) throw new Error("Missing required plugin state");
  return value;
}

import { view } from "zellij:workflow-view";
type Worktree = { path: string; head: string; branch?: string };
type Checkout = Worktree & { primary: string };
type Vm = Readonly<{
  session?: unknown;
  name?: unknown;
  vm_id?: unknown;
  worktree?: unknown;
  metrics?: Readonly<{ generation?: unknown }>;
}>;
let acting = false,
  busy = false,
  confirming = false,
  removedWorktree = false,
  status = "Agent exited.",
  workflow = false;
let checkout: Checkout | undefined,
  directory: string | undefined,
  launchToken: string | undefined,
  pane: number | undefined;
let firecracker:
  | {
      env: Record<string, string>;
      executable: string;
      identity?: string;
      name: string;
      session: string;
    }
  | undefined;

async function command(
  argv: readonly string[],
  envVariables: Readonly<Record<string, string>> = {},
): Promise<string> {
  const result = await z.runCommand({
    command: [...argv],
    cwd: "/",
    envVariables: { ...envVariables },
    maxBytesReturned: 1048576,
  });
  const stdout = z.decodeUtf8(result.stdout);
  if (result.exitCode !== 0) {
    const stderr = z.decodeUtf8(result.stderr).trim();
    throw new Error(
      `${argv[0]} failed (${result.exitCode ?? "signal"}): ${stderr || stdout.trim()}`,
    );
  }
  return stdout;
}
function worktrees(output: string): Worktree[] {
  const result: Worktree[] = [];
  let current: Partial<Worktree> = {};
  const finish = (): void => {
    if (current.path && current.head)
      result.push({
        path: current.path,
        head: current.head,
        ...(current.branch ? { branch: current.branch } : {}),
      });
    current = {};
  };
  for (const field of output.split("\0")) {
    if (!field) finish();
    else if (field.startsWith("worktree ")) current.path = field.slice(9);
    else if (field.startsWith("HEAD ")) current.head = field.slice(5);
    else if (field.startsWith("branch refs/heads/")) current.branch = field.slice(18);
  }
  finish();
  return result;
}
async function inspectCheckout(): Promise<void> {
  try {
    const path = (
      await command([
        "git",
        "-C",
        required(directory),
        "rev-parse",
        "--path-format=absolute",
        "--show-toplevel",
      ])
    ).trim();
    const listed = worktrees(
      await command(["git", "-C", path, "worktree", "list", "--porcelain", "-z"]),
    );
    const found = listed.find((entry) => entry.path === path);
    if (found && listed[0] && found.path !== listed[0].path)
      checkout = { ...found, primary: listed[0].path };
  } catch {
    // A non-repository or unavailable identity simply has no destructive action.
  }
  z.requestRender();
}
function workflowHidden(): boolean {
  return confirming || busy || status.startsWith("Deletion failed:");
}
function close(): void {
  if (pane != null) z.command("CloseTerminalPane", pane);
  z.command("CloseSelf");
}
function vmIdentity(vm: Readonly<Vm>): string {
  const generation = vm.metrics?.generation;
  if (
    typeof vm.session !== "string" ||
    typeof vm.name !== "string" ||
    typeof vm.vm_id !== "string" ||
    !vm.vm_id ||
    typeof generation !== "string" ||
    !generation ||
    typeof vm.worktree !== "string"
  )
    throw new Error("Firecracker did not report a complete VM identity.");
  return JSON.stringify({
    session: vm.session,
    name: vm.name,
    vm_id: vm.vm_id,
    generation,
    worktree: vm.worktree,
  });
}
async function listVms(): Promise<Vm[]> {
  const configured = required(firecracker);
  const parsed: unknown = JSON.parse(
    await command([configured.executable, "firecracker", "list", "--json"], configured.env),
  );
  if (
    typeof parsed !== "object" ||
    parsed == null ||
    !("vms" in parsed) ||
    !Array.isArray(parsed.vms)
  )
    throw new Error("Firecracker returned an invalid VM listing.");
  return parsed.vms.filter((entry): entry is Vm => typeof entry === "object" && entry != null);
}
async function closeVm(): Promise<void> {
  const configured = required(firecracker);
  const original = required(checkout);
  if (!configured.identity) {
    const before = await listVms();
    const vm = before.find(
      (entry) => entry.session === configured.session && entry.name === configured.name,
    );
    if (vm && vm.worktree !== original.path)
      throw new Error("The Firecracker VM identity changed; deletion was refused.");
    if (!vm) {
      const remaining = before.filter((entry) => entry.worktree === original.path);
      if (remaining.length > 0)
        throw new Error(
          `Worktree is still leased by ${remaining.map((entry) => String(entry.session)).join(", ")}.`,
        );
      throw new Error("The Firecracker VM lease disappeared; deletion was refused.");
    }
    configured.identity = vmIdentity(vm);
  }
  await command(
    [
      configured.executable,
      "firecracker",
      "close",
      configured.name,
      "--session",
      configured.session,
      "--expected-identity",
      configured.identity,
      "--expected-worktree",
      original.path,
      "--expected-head",
      original.head,
      ...(original.branch ? ["--expected-branch", original.branch] : []),
    ],
    configured.env,
  );
}
async function deleteCheckout(): Promise<void> {
  const original = required(checkout);
  if (!removedWorktree && firecracker) {
    await closeVm();
    removedWorktree = true;
  } else if (!removedWorktree) {
    const listed = worktrees(
      await command(["git", "-C", original.primary, "worktree", "list", "--porcelain", "-z"]),
    );
    const found = listed.find((entry) => entry.path === original.path);
    if (!found) throw new Error("The worktree disappeared; deletion was refused.");
    if (
      found.path === listed[0]?.path ||
      found.head !== original.head ||
      found.branch !== original.branch
    )
      throw new Error("The worktree identity changed; deletion was refused.");
    await command([
      "git",
      "-C",
      original.primary,
      "worktree",
      "remove",
      "--force",
      "--",
      original.path,
    ]);
    removedWorktree = true;
  }
  if (original.branch) {
    await command([
      "git",
      "-C",
      original.primary,
      "update-ref",
      "-d",
      `refs/heads/${original.branch}`,
      original.head,
    ]);
  }
  close();
}
async function remove(): Promise<void> {
  busy = true;
  status = "Cancelling workflow and stopping owned operations...";
  z.requestRender();
  try {
    if (launchToken) {
      const deadline = Date.now() + 60000;
      for (;;) {
        if (Date.now() >= deadline)
          throw new Error("Cleanup is still pending; retry after reconciliation.");
        // oxlint-disable-next-line eslint/no-await-in-loop -- Cleanup readiness must precede deletion.
        const result = await z.rpc.call({
          pluginUrl: "zellij:swamp-bridge",
          name: "swamp",
          timeoutMs: 60000,
          payload: { method: "cleanup_prepare", work: launchToken },
        });
        if (
          result != null &&
          typeof result === "object" &&
          !Array.isArray(result) &&
          result["ready"] === true
        )
          break;
        // oxlint-disable-next-line eslint/no-await-in-loop -- Poll readiness without overlapping requests.
        await z.sleep(250);
      }
    }
    status = "Deleting worktree...";
    z.requestRender();
    await deleteCheckout();
  } catch (error) {
    status = `Deletion failed: ${String(error)}`;
    confirming = false;
    busy = false;
    z.requestRender();
  }
}

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
  if (busy) return;
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
      status = "No local control checkout is available.";
      return;
    }
    const inspect = (): void => {
      if (config["agent_target"] !== "local" && config["agent_target"] !== "firecracker") {
        status = "Checkout identity is unavailable.";
        return;
      }
      if (config["agent_target"] === "firecracker") {
        const executable = config["agent_zellij_executable"] ?? z.serverBinary;
        const name = config["agent_firecracker_vm"];
        const session = config["agent_session"];
        if (!executable || !name || !session) {
          status = "Firecracker identity is unavailable.";
          return;
        }
        const env: Record<string, string> = {};
        if (config["agent_zellij_config_file"])
          env["ZELLIJ_CONFIG_FILE"] = config["agent_zellij_config_file"];
        if (config["agent_zellij_config_dir"])
          env["ZELLIJ_CONFIG_DIR"] = config["agent_zellij_config_dir"];
        firecracker = { env, executable, name, session };
      }

      void inspectCheckout().catch((error: unknown) => {
        status = String(error);
        z.requestRender();
      });
    };
    let projectWorkflow: string | null = null;
    try {
      const snapshot = z.projectConfig.read(directory);
      if (snapshot.error) {
        status = snapshot.error;
      } else projectWorkflow = snapshot.config.workflow;
    } catch (error) {
      status = String(error);
    }
    inspect();
    if (projectWorkflow) {
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
      void launch(config).catch((error: unknown) => {
        view.status = String(error);
        z.requestRender();
      });
    }
  },
  // oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- Workflow and fallback key handling share mutable lifecycle state.
  update(event: Event) {
    if (typeof event === "object" && "Key" in event) {
      if (busy) return true;
      if (event.Key.bare_key === "Delete" && checkout) {
        if (confirming)
          void remove().catch((error: unknown) => {
            z.log(String(error));
          });
        else confirming = true;
        z.requestRender();
        return true;
      }
      if (confirming) {
        if (event.Key.bare_key === "Esc") confirming = false;
        z.requestRender();
        return true;
      }
      if (event.Key.bare_key === "Esc") {
        close();
        return true;
      }
    }
    if (workflow) {
      if (
        typeof event === "object" &&
        ("Key" in event || "Mouse" in event || "PastedText" in event) &&
        workflowHidden()
      )
        return true;
      return view.update(event);
    }
    if (typeof event !== "object" || !("Key" in event)) return false;
    if (event.Key.bare_key === "Enter") {
      if (pane != null) z.command("RerunCommandPane", pane);
      z.command("CloseSelf");
    }
    return true;
  },
  pipe: (message) => {
    view.pipe(message);
  },
  render(rows, cols) {
    const destructive = checkout ? "Delete: Delete worktree & branch and close pane" : "";
    if (workflow && !workflowHidden())
      return (
        view.render(Math.max(0, rows - 1), cols) +
        z.ui
          .text(
            z.text.truncate(
              "Esc: close pane  " +
                (destructive || "Enter: activate button. Restart/Fork: cancel run and continue."),
              cols,
            ),
          )
          .dim()
          .render({ x: 0, y: Math.max(0, rows - 1), width: cols, height: 1 })
      );
    return [
      ...status.split("\n").filter(Boolean),
      `Directory: ${directory ?? "unknown"}`,
      ...(confirming && checkout
        ? [
            `Worktree: ${checkout.path}`,
            `Branch: ${checkout.branch ?? "detached HEAD (checkout only)"}`,
            ...(firecracker ? [`VM: ${firecracker.name}`] : []),
            "This cancels the workflow and removes uncommitted work.",
            "Delete again: confirm deletion. Esc: cancel.",
          ]
        : ["Enter: restart agent  Esc: close pane", destructive]),
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
