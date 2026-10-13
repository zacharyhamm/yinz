// Keep the exited pane available for restart or guarded checkout cleanup.
import z, { definePlugin, type Event } from "yinz:plugin";
function required<T>(value: T): NonNullable<T> {
  if (value == null) throw new Error("Missing required plugin state");
  return value;
}

type Worktree = { path: string; head: string; branch?: string };
type Checkout = Worktree & { primary: string };
let busy = false,
  confirmationVisible = false,
  confirming = false,
  deletionFailed = false,
  removedWorktree = false,
  status = "Agent exited.";
let checkout: Checkout | undefined,
  directory: string | undefined,
  failureStatus: string | undefined,
  pane: number | undefined;
async function command(
  argv: readonly string[],
  envVariables: Readonly<Record<string, string>> = {},
  timeoutMs?: number,
): Promise<string> {
  const pending = z.runCommand({
    command: [...argv],
    cwd: "/",
    envVariables: { ...envVariables },
    maxBytesReturned: 1048576,
  });
  let timedOut = false;
  const timer =
    timeoutMs == null
      ? undefined
      : z.after(timeoutMs, () => {
          timedOut = true;
          // Cancellation stops waiting and ignores late results; Git keeps running.
          pending.cancel();
        });
  const result = await pending
    .catch((error: unknown) => {
      if (timedOut) throw new Error(`${argv[0]} timed out after ${timeoutMs} ms`);
      throw error;
    })
    .finally(() => timer?.cancel());
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
// oxlint-disable-next-line eslint/max-lines-per-function -- Keep identity and preservation checks before both destructive cleanup paths.
async function deleteCheckout(): Promise<void> {
  const original = required(checkout);
  if (!removedWorktree) {
    const config = z.projectConfig.read(original.path);
    if (config.error) throw new Error(config.error);
    const base = config.config.default_branch ?? "main";
    const references = [`refs/heads/${base}`];
    const diagnostics: string[] = [];
    try {
      await command(
        [
          "git",
          "-C",
          original.primary,
          "fetch",
          "--no-tags",
          "origin",
          `+refs/heads/${base}:refs/remotes/origin/${base}`,
        ],
        {},
        10000,
      );
      references.push(`refs/remotes/origin/${base}`);
    } catch (error) {
      diagnostics.push(`Fetch origin/${base}: ${String(error)}`);
    }
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
    let merged = false;
    for (const reference of references) {
      try {
        // oxlint-disable-next-line eslint/no-await-in-loop -- Stop at the first successful ancestry proof.
        const unmerged = await command([
          "git",
          "-C",
          original.primary,
          "rev-list",
          "--max-count=1",
          `${reference}..${original.head}`,
          "--",
        ]);
        if (!unmerged.trim()) {
          merged = true;
          break;
        }
      } catch (error) {
        diagnostics.push(`${reference}: ${String(error)}`);
      }
    }
    if (!merged)
      throw new Error(
        `Cannot verify commits merged into local ${base} or freshly fetched origin/${base}; worktree and branch preserved.\n${diagnostics.join("\n")}`,
      );
    await command(["git", "-C", original.primary, "worktree", "remove", "--", original.path]);
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
  status = "Deleting worktree...";
  z.requestRender();
  try {
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

export default definePlugin({
  load(config) {
    pane = config["agent_pane_id"] == null ? undefined : Number(config["agent_pane_id"]);
    directory = config["agent_cwd"];
    z.subscribe(["Key"]);
    if (!directory || config["agent_target"] === "ssh") {
      status = "No local checkout is available.";
      return;
    }
    const inspect = (): void => {
      if (config["agent_target"] !== "local" && config["agent_target"] !== "bubblewrap") {
        status = "Checkout identity is unavailable.";
        return;
      }

      void inspectCheckout().catch((error: unknown) => {
        status = String(error);
        z.requestRender();
      });
    };
    try {
      const snapshot = z.projectConfig.read(directory);
      if (snapshot.error) {
        status = snapshot.error;
      }
    } catch (error) {
      status = String(error);
    }
    inspect();
  },
  // oxlint-disable-next-line eslint/complexity, typescript/prefer-readonly-parameter-types -- Keep cleanup and pane shortcuts together in their input precedence order.
  update(event: Event) {
    if (typeof event === "object" && "Key" in event) {
      if (busy) return true;
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
          deletionFailed = removedWorktree;
          status = deletionFailed ? (failureStatus ?? "Cleanup is incomplete.") : "Agent exited.";
        }
        z.requestRender();
        return true;
      }
      if (key === "Esc") {
        close();
        return true;
      }
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
  // oxlint-disable-next-line eslint/max-lines-per-function -- Keep confirmation visibility and its deletion guard together with the rendered controls.
  render(rows, cols) {
    const destructive = checkout ? "Delete: Delete worktree & branch and close pane" : "";
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
        "Deletion requires a clean worktree and commits merged into the local base branch or its freshly fetched origin branch.",
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
              ...(removedWorktree ? [] : ["Enter: restart agent"]),
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
