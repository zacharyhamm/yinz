// Attach a project-owned Swamp workflow. Retain the source until Leave.
import z, { definePlugin, type Event } from "yinz:plugin";
function required<T>(value: T): NonNullable<T> {
  if (value == null) throw new Error("Missing required plugin state");
  return value;
}

import { view } from "./workflow-view.ts";
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
  cleanupStarted = false,
  confirmationVisible = false,
  confirming = false,
  deletionFailed = false,
  removedWorktree = false,
  status = "Agent exited.",
  workflow = false;
let checkout: Checkout | undefined,
  directory: string | undefined,
  failureStatus: string | undefined,
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
  return confirming || busy || deletionFailed;
}
async function confirmCheckout(): Promise<void> {
  busy = true;
  confirmationVisible = false;
  status = "Checking current checkout before deletion...";
  z.requestRender();
  try {
    if (!removedWorktree) {
      const original = required(checkout);
      const listed = worktrees(
        await command(["git", "-C", original.primary, "worktree", "list", "--porcelain", "-z"]),
      );
      const found = listed.find((entry) => entry.path === original.path);
      if (!found || found.path === listed[0]?.path)
        throw new Error("The secondary worktree is unavailable; deletion was refused.");
      checkout = { ...found, primary: original.primary };
    }
    confirming = true;
    deletionFailed = false;
  } catch (error) {
    status = `Deletion failed: ${String(error)}`;
    failureStatus = status;
    deletionFailed = true;
  } finally {
    busy = false;
    z.requestRender();
  }
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
// oxlint-disable-next-line eslint/max-lines-per-function -- Keep identity and preservation checks before both destructive cleanup paths.
async function deleteCheckout(): Promise<void> {
  const original = required(checkout);
  if (!removedWorktree) {
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
    const config = z.projectConfig.read(original.path);
    if (config.error) throw new Error(config.error);
    const base = config.config.default_branch ?? "main";
    const changes = await command([
      "git",
      "-C",
      original.path,
      "status",
      "--porcelain",
      "--untracked-files=all",
    ]);
    if (changes.trim())
      throw new Error(
        "Worktree has uncommitted changes (including untracked files); worktree and branch preserved.",
      );
    const unmerged = await command([
      "git",
      "-C",
      original.primary,
      "rev-list",
      "--max-count=1",
      `refs/heads/${base}..${original.head}`,
      "--",
    ]);
    if (unmerged.trim())
      throw new Error(
        `Worktree contains commits not merged into ${base}; worktree and branch preserved.`,
      );
    if (firecracker) await closeVm();
    else await command(["git", "-C", original.primary, "worktree", "remove", "--", original.path]);
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
      cleanupStarted = true;
      const deadline = Date.now() + 60000;
      for (;;) {
        if (Date.now() >= deadline)
          throw new Error("Cleanup is still pending; retry after reconciliation.");
        // oxlint-disable-next-line eslint/no-await-in-loop -- Cleanup readiness must precede deletion.
        const result = await z.rpc.call({
          pluginUrl: "yinz:swamp-bridge",
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
    failureStatus = status;
    deletionFailed = true;
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
  if (cleanupStarted) return;
  const token = config["agent_exit_token"];
  if (!token) throw new Error("Missing host exit correlation token");
  view.watch(token);
  try {
    await z.rpc.call({
      pluginUrl: "yinz:swamp-bridge",
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
        const executable = config["agent_yinz_executable"] ?? z.serverBinary;
        const name = config["agent_firecracker_vm"];
        const session = config["agent_session"];
        if (!executable || !name || !session) {
          status = "Firecracker identity is unavailable.";
          return;
        }
        const env: Record<string, string> = {};
        if (config["agent_yinz_config_file"])
          env["YINZ_CONFIG_FILE"] = config["agent_yinz_config_file"];
        if (config["agent_yinz_config_dir"])
          env["YINZ_CONFIG_DIR"] = config["agent_yinz_config_dir"];
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
        view.action = action;
        view.noticeError = false;
        view.notice = action === "restart" ? "Restarting agent..." : "Opening agent picker...";
        z.requestRender();
        void z.rpc
          .call({
            pluginUrl: "yinz:swamp-bridge",
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
  // oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function, typescript/prefer-readonly-parameter-types -- Keep cleanup, editing and pane shortcuts together in their input precedence order.
  update(event: Event) {
    if (
      acting &&
      typeof event === "object" &&
      ("Key" in event || "Mouse" in event || "PastedText" in event)
    )
      return true;
    if (typeof event === "object" && "Key" in event) {
      if (busy) return true;
      if (workflow && !workflowHidden() && view.focusedControl() === "answer")
        return view.update(event);
      const key = event.Key.key_modifiers.length === 0 ? event.Key.bare_key : undefined;
      if (key === "Delete" && checkout) {
        if (confirming && confirmationVisible)
          void remove().catch((error: unknown) => {
            z.log(String(error));
          });
        else if (!confirming)
          void confirmCheckout().catch((error: unknown) => {
            z.log(String(error));
          });
        z.requestRender();
        return true;
      }
      if (confirming) {
        if (key === "Esc") {
          confirming = false;
          deletionFailed = cleanupStarted || removedWorktree;
          status = deletionFailed ? (failureStatus ?? "Cleanup is incomplete.") : "Agent exited.";
        }
        z.requestRender();
        return true;
      }
      if (key === "Esc") {
        close();
        return true;
      }
      if (deletionFailed && key === "Enter" && !cleanupStarted && workflow) {
        deletionFailed = false;
        z.requestRender();
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
    if (
      event.Key.bare_key === "Enter" &&
      event.Key.key_modifiers.length === 0 &&
      !removedWorktree
    ) {
      if (pane != null) z.command("RerunCommandPane", pane);
      z.command("CloseSelf");
    }
    return true;
  },
  pipe: (message) => {
    view.pipe(message);
  },
  // oxlint-disable-next-line eslint/complexity, eslint/max-lines-per-function -- Keep confirmation visibility and its deletion guard together with the rendered controls.
  render(rows, cols) {
    const destructive = checkout ? "Delete: Delete worktree & branch and close pane" : "";
    if (workflow && !workflowHidden())
      return (
        view.render(Math.max(0, rows - 1), cols) +
        z.ui
          .text(
            z.text.truncate(
              view.focusedControl() === "answer"
                ? "Esc: finish editing  Tab: actions"
                : "Esc: close pane  " +
                    (destructive ||
                      "Enter: activate button. Restart/Fork: cancel run and continue."),
              cols,
            ),
          )
          .dim()
          .render({ x: 0, y: Math.max(0, rows - 1), width: cols, height: 1 })
      );
    const wrap = (lines: readonly string[]): string[] =>
      lines.flatMap((line) => z.text.wrap(z.text.sanitize(line), Math.max(1, cols)));
    let lines: string[];
    if (confirming && checkout) {
      lines = wrap([
        removedWorktree
          ? "Worktree removed. Delete remaining branch?"
          : "Delete worktree and branch?",
        `Worktree: ${checkout.path}`,
        `Branch: ${checkout.branch ?? "detached HEAD (checkout only)"}`,
        `Revision: ${checkout.head}`,
        ...(firecracker ? [`VM: ${firecracker.name}`] : []),
        "This cancels the workflow.",
        "Deletion requires a clean worktree and commits merged into the base branch.",
        "Delete again: confirm deletion. Esc: cancel.",
      ]);
      confirmationVisible = cols > 0 && lines.length <= rows;
      if (!confirmationVisible) {
        const footer = wrap(["Deletion disabled: enlarge pane to review.", "Esc: cancel."]);
        lines = [...lines.slice(0, Math.max(0, rows - footer.length)), ...footer];
      }
    } else {
      confirmationVisible = false;
      const footer = busy
        ? ["Waiting for this operation to finish..."]
        : deletionFailed
          ? [
              removedWorktree
                ? "Worktree removed; branch cleanup remains."
                : "Worktree cleanup is incomplete.",
              ...(workflow && !cleanupStarted ? ["Enter: back to workflow"] : []),
              ...(!workflow && !removedWorktree ? ["Enter: restart agent"] : []),
              "Delete: retry cleanup  Esc: close pane",
            ]
          : ["Enter: restart agent  Esc: close pane", destructive];
      const controls = wrap(footer.filter(Boolean));
      lines = [
        ...wrap([
          ...status.split("\n").filter(Boolean),
          `Directory: ${directory ?? "unknown"}`,
        ]).slice(0, Math.max(0, rows - controls.length)),
        ...controls,
      ];
    }
    return lines
      .slice(0, rows)
      .map((line, y) =>
        z.ui
          .text(z.text.truncate(z.text.sanitize(line), cols))
          .render({ x: 0, y, width: cols, height: 1 }),
      )
      .join("");
  },
});
