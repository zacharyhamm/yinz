import { test, loadPlugin, assert, assertEquals } from "yinz:testing";

const head = "0123456789012345678901234567890123456789";
const listing = (secondary = true): string =>
  `worktree /repo\0HEAD ${head}\0branch refs/heads/main\0\0${
    secondary ? `worktree /repo/topic\0HEAD ${head}\0branch refs/heads/topic\0\0` : ""
  }`;
function bytes(value: string): number[] {
  return [...value].map((character) => character.codePointAt(0) ?? 0);
}
function lastRunCommand(plugin: Awaited<ReturnType<typeof loadPlugin>>) {
  const captured = [...plugin.commands]
    .reverse()
    .find((entry) => typeof entry === "object" && entry != null && "RunCommand" in entry);
  assert(typeof captured === "object" && captured != null && "RunCommand" in captured);
  return captured;
}
async function reply(
  plugin: Awaited<ReturnType<typeof loadPlugin>>,
  stdout: string,
  exitCode = 0,
  stderr = "",
): Promise<void> {
  const captured = lastRunCommand(plugin);
  const payload = captured.RunCommand;
  assert(Array.isArray(payload));
  const context = payload[3];
  assert(typeof context === "object" && context != null);
  await plugin.event({ RunCommandResult: [exitCode, bytes(stdout), bytes(stderr), context] });
}
async function checkout(
  configuration: Record<string, string> = {},
  listingOutput = listing(),
): Promise<Awaited<ReturnType<typeof loadPlugin>>> {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: {
      agent_cwd: "/repo/topic",
      agent_pane_id: "7",
      agent_target: "local",
      ...configuration,
    },
    replies: {
      ProjectConfigRead: { config: { default_branch: "main" }, error: null },
      RunCommand: null,
      RequestPluginPermissions: null,
      CloseSelf: null,
      RerunCommandPane: null,
      CloseTerminalPane: null,
    },
  });
  await reply(plugin, "/repo/topic\n");
  await reply(plugin, listingOutput);
  return plugin;
}

async function prepareDeletion(
  plugin: Awaited<ReturnType<typeof loadPlugin>>,
  listingOutput = listing(),
): Promise<void> {
  const before = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  if (
    plugin.commands
      .slice(before)
      .some((entry) => typeof entry === "object" && entry != null && "RunCommand" in entry)
  )
    await reply(plugin, listingOutput);
  await plugin.render(24, 200);
}
async function confirmDeletion(
  plugin: Awaited<ReturnType<typeof loadPlugin>>,
  listingOutput = listing(),
): Promise<void> {
  await prepareDeletion(plugin, listingOutput);
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  if (JSON.stringify(lastRunCommand(plugin)).includes('"fetch"'))
    await reply(plugin, "", 128, "origin unavailable");
}

async function cleanCheckout(plugin: Awaited<ReturnType<typeof loadPlugin>>): Promise<void> {
  assert(
    JSON.stringify(plugin.commands.at(-1)).includes(
      '"status","--porcelain","--untracked-files=all"',
    ),
  );
  await reply(plugin, "");
  assert(JSON.stringify(plugin.commands.at(-1)).includes('"rev-list","--max-count=1"'));
  await reply(plugin, "");
}

for (const scenario of [
  { name: "fresh remote proof", local: head, remote: "" },
  { name: "local proof with remote behind", local: "", remote: head },
  { name: "neither destination merged", local: head, remote: head },
  { name: "failed fetch with local proof", local: "", fetchFails: true },
  { name: "failed fetch excludes stale remote proof", local: head, fetchFails: true },
  { name: "missing local base with remote proof", local: "", localFails: true, remote: "" },
  { name: "Git verification failure", local: "", localFails: true, remoteFails: true },
]) {
  test(`agent-exit ${scenario.name}`, async () => {
    const plugin = await checkout();
    await prepareDeletion(plugin);
    assert(!JSON.stringify(plugin.commands).includes('"fetch"'));
    await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
    assert(
      JSON.stringify(lastRunCommand(plugin)).includes(
        '"fetch","--no-tags","origin","+refs/heads/main:refs/remotes/origin/main"',
      ),
    );
    await reply(plugin, "", scenario.fetchFails ? 128 : 0, "fetch unavailable");
    await reply(plugin, listing());
    await reply(plugin, "");
    await reply(plugin, scenario.local, scenario.localFails ? 128 : 0, "local unavailable");
    const localProof = !scenario.local && !scenario.localFails;
    if (!localProof && !scenario.fetchFails) {
      assert(JSON.stringify(plugin.commands.at(-1)).includes(`refs/remotes/origin/main..${head}`));
      await reply(
        plugin,
        scenario.remote ?? "",
        scenario.remoteFails ? 128 : 0,
        "remote unavailable",
      );
    }
    const merged =
      localProof || (!scenario.fetchFails && !scenario.remoteFails && !scenario.remote);
    if (merged) {
      await reply(plugin, "");
      await reply(plugin, "");
      assertEquals(plugin.commands.at(-1), "CloseSelf");
    } else {
      const rendered = await plugin.render(30, 200);
      assert(rendered.includes("worktree and branch preserved"));
      assert(rendered.includes("local main or freshly fetched origin/main"));
      if (scenario.fetchFails) assert(rendered.includes("fetch unavailable"));
      assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
      assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
    }
    if (scenario.fetchFails)
      assert(!JSON.stringify(plugin.commands).includes(`refs/remotes/origin/main..${head}`));
  });
}
for (const localProof of [true, false]) {
  test(`agent-exit fetch deadline ${localProof ? "allows local cleanup" : "preserves without local proof"}`, async () => {
    const plugin = await checkout();
    await prepareDeletion(plugin);
    await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
    const fetch = lastRunCommand(plugin).RunCommand;
    assert(Array.isArray(fetch));
    await plugin.advance(9999);
    assertEquals(lastRunCommand(plugin).RunCommand, fetch);
    const pending = plugin.commands.length;
    assert(!(await plugin.render()).includes("Deletion failed"));
    await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
    assertEquals(plugin.commands.length, pending);
    await plugin.advance(1);
    assert(JSON.stringify(lastRunCommand(plugin)).includes('"worktree","list"'));
    const verifying = plugin.commands.length;
    await plugin.event({ RunCommandResult: [0, [], [], fetch[3]] });
    assertEquals(plugin.commands.length, verifying);
    await reply(plugin, listing());
    await reply(plugin, "");
    assert(JSON.stringify(lastRunCommand(plugin)).includes(`refs/heads/main..${head}`));
    await reply(plugin, localProof ? "" : head);
    assert(!JSON.stringify(plugin.commands).includes(`refs/remotes/origin/main..${head}`));
    if (localProof) {
      assert(JSON.stringify(lastRunCommand(plugin)).includes('"worktree","remove"'));
      await reply(plugin, "");
      await reply(plugin, "");
      assertEquals(plugin.commands.at(-1), "CloseSelf");
    } else {
      const failure = await plugin.render(30, 200);
      assert(failure.includes("Fetch origin/main: Error: git timed out after 10000 ms"));
      assert(failure.includes("worktree and branch preserved"));
      assert(failure.includes("Delete: retry cleanup  Esc: close pane"));
      assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
      assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
      const preserved = plugin.commands.length;
      await plugin.event({ RunCommandResult: [0, [], [], fetch[3]] });
      assertEquals(plugin.commands.length, preserved);
      assertEquals(await plugin.render(30, 200), failure);
      await prepareDeletion(plugin);
      assert((await plugin.render(30, 200)).includes("Delete again"));
      await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
      assert(!plugin.commands.includes("CloseSelf"));
      await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
      assertEquals(plugin.commands.slice(-2), [{ CloseTerminalPane: 7 }, "CloseSelf"]);
    }
  });
}
for (const exitCode of [0, 128]) {
  test(`agent-exit cancels fetch deadline after exit ${exitCode}`, async () => {
    const plugin = await checkout();
    await prepareDeletion(plugin);
    await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
    await reply(plugin, "", exitCode);
    const count = plugin.commands.length;
    // A live deadline would rearm SetTimeout on this early timer event.
    await plugin.advance(9999);
    assertEquals(plugin.commands.length, count);
    await plugin.advance(1);
    assertEquals(plugin.commands.length, count);
    await reply(plugin, listing());
    await cleanCheckout(plugin);
    await reply(plugin, "");
    await reply(plugin, "");
    assertEquals(plugin.commands.at(-1), "CloseSelf");
  });
}
for (const change of ["head", "branch", "dirty"]) {
  test(`agent-exit refuses ${change} changes during fetch`, async () => {
    const plugin = await checkout();
    await prepareDeletion(plugin);
    await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
    await reply(plugin, "");
    await reply(
      plugin,
      change === "head"
        ? listing().replaceAll(head, "1".repeat(40))
        : change === "branch"
          ? listing().replace("refs/heads/topic", "refs/heads/other")
          : listing(),
    );
    if (change === "dirty") await reply(plugin, "?? new-file\n");
    assert(
      (await plugin.render()).includes(
        change === "dirty" ? "uncommitted changes" : "identity changed",
      ),
    );
    assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
    assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
  });
}

for (const [key, commands] of [
  ["Enter", ["RerunCommandPane", "CloseSelf"]],
  ["Esc", ["CloseTerminalPane", "CloseSelf"]],
] as const) {
  test(`agent-exit fallback ${key}`, async () => {
    const plugin = await loadPlugin("./agent-exit.ts", {
      configuration: { agent_pane_id: "7" },
      replies: { CloseSelf: null, RerunCommandPane: null, CloseTerminalPane: null },
    });
    assert((await plugin.render()).includes("restart agent"));
    await plugin.event({ Key: { bare_key: key, key_modifiers: [] } });
    assertEquals(
      plugin.commands
        .slice(-commands.length)
        .map((command) =>
          typeof command === "string" ? command : Object.keys(command as object)[0],
        ),
      [...commands],
    );
  });
}
test("agent-exit offers no deletion without a secondary worktree", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_pane_id: "7" },
  });
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(
    plugin.commands.filter((command) => command === "CloseSelf"),
    [],
  );
  assert(!(await plugin.render()).includes("Delete worktree"));
});
test("agent-exit offers no deletion for a primary checkout", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo", agent_target: "local" },
    replies: {
      ProjectConfigRead: { config: { default_branch: "main" }, error: null },
      RunCommand: null,
    },
  });
  await reply(plugin, "/repo\n");
  await reply(plugin, listing(false));
  assert(!(await plugin.render()).includes("Delete worktree"));
});
test("agent-exit offers no deletion outside a repository", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo", agent_target: "local" },
    replies: {
      ProjectConfigRead: { config: { default_branch: "main" }, error: null },
      RunCommand: null,
    },
  });
  await reply(plugin, "", 128, "not a repository");
  assert(!(await plugin.render()).includes("Delete worktree"));
});
test("agent-exit shows project configuration errors while inspecting Git", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "local" },
    replies: {
      ProjectConfigRead: { config: { default_branch: "main" }, error: "malformed project config" },
      RunCommand: null,
    },
  });
  await reply(plugin, "/repo/topic\n");
  await reply(plugin, listing());
  assert((await plugin.render()).includes("malformed project config"));
  assert(JSON.stringify(plugin.commands).includes("RunCommand"));
  assert((await plugin.render()).includes("Delete worktree"));
});
test("agent-exit shows thrown project reads while inspecting Git", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "local" },
    replies: { RunCommand: null },
  });
  await reply(plugin, "/repo/topic\n");
  await reply(plugin, listing());
  assert((await plugin.render()).includes("unstubbed host operation ProjectConfigRead"));
  assert(JSON.stringify(plugin.commands).includes("RunCommand"));
  assert((await plugin.render()).includes("Delete worktree"));
});
test("agent-exit confirms and cancels secondary worktree deletion", async () => {
  const plugin = await checkout();
  assert((await plugin.render()).includes("Delete: Delete worktree & branch and close pane"));
  await prepareDeletion(plugin);
  const confirmation = await plugin.render();
  assert(confirmation.includes("/repo/topic"));
  assert(confirmation.includes("topic"));
  await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
  assert(!(await plugin.render()).includes("Delete again"));
});
for (const target of ["local", "bubblewrap"]) {
  test(`agent-exit revalidates then removes the ${target} worktree and branch`, async () => {
    const plugin = await checkout({ agent_target: target });
    await confirmDeletion(plugin);
    await reply(plugin, listing());
    await cleanCheckout(plugin);
    await reply(plugin, "");
    await reply(plugin, "");
    const commands = JSON.stringify(plugin.commands);
    assert(commands.includes('"worktree","remove","--","/repo/topic"'));
    assert(commands.includes(`"update-ref","-d","refs/heads/topic","${head}"`));
    assertEquals(plugin.commands.at(-2), { CloseTerminalPane: 7 });
    assertEquals(plugin.commands.at(-1), "CloseSelf");
  });
}
for (const target of ["local", "bubblewrap"]) {
  for (const changes of [" M tracked\n", "?? untracked\n", "M  staged\n"]) {
    test(`agent-exit preserves ${target} dirty worktree and branch: ${changes.trim()}`, async () => {
      const plugin = await checkout({
        agent_target: target,
      });
      await confirmDeletion(plugin);
      await reply(plugin, listing());
      await reply(plugin, changes);
      assert((await plugin.render(24, 200)).includes("uncommitted changes"));
      const commands = JSON.stringify(plugin.commands);
      assert(!commands.includes('"worktree","remove"'));
      assert(!commands.includes('"update-ref"'));
      assert(!plugin.commands.includes("CloseSelf"));
    });
  }
  test(`agent-exit preserves ${target} commits not merged into the configured base`, async () => {
    const plugin = await checkout({
      agent_target: target,
    });
    plugin.stub("ProjectConfigRead", {
      config: { default_branch: "trunk" },
      error: null,
    });
    await confirmDeletion(plugin);
    assert(JSON.stringify(plugin.commands).includes("+refs/heads/trunk:refs/remotes/origin/trunk"));
    await reply(plugin, listing());
    await reply(plugin, "");
    assert(JSON.stringify(plugin.commands.at(-1)).includes(`refs/heads/trunk..${head}`));
    await reply(plugin, head + "\n");
    assert((await plugin.render(24, 200)).includes("merged into local trunk"));
    const commands = JSON.stringify(plugin.commands);
    assert(!commands.includes('"worktree","remove"'));
    assert(!commands.includes('"update-ref"'));
    assert(!plugin.commands.includes("CloseSelf"));
  });
}
test("agent-exit refuses branch deletion when the worktree disappeared externally", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing(false));
  assert((await plugin.render()).includes("worktree disappeared"));
  assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
});
test("agent-exit keeps diagnostics visible and retries after partial cleanup", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "", 1, "branch is locked");
  const failure = await plugin.render();
  assert(failure.includes("branch is locked"));
  assert(failure.includes("Worktree removed; branch cleanup remains."));
  assert(!failure.includes("restart agent"));
  const count = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  await prepareDeletion(plugin);
  await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
  assert((await plugin.render()).includes("branch is locked"));
  await confirmDeletion(plugin);
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit treats an already absent branch as completed", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit refuses a branch whose head changed", async () => {
  const plugin = await checkout();
  await confirmDeletion(plugin);
  await reply(plugin, listing());
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "", 128, "cannot lock ref: is at another head but expected original");
  assert((await plugin.render()).includes("cannot lock ref"));
  assert(!plugin.commands.includes("CloseSelf"));
});
test("agent-exit removes a detached secondary checkout without deleting a branch", async () => {
  const detached = listing().replace("branch refs/heads/topic", "detached");
  const plugin = await checkout({}, detached);
  await prepareDeletion(plugin, detached);
  assert((await plugin.render()).includes("detached HEAD"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, "", 128, "origin unavailable");
  await reply(plugin, detached);
  await cleanCheckout(plugin);
  await reply(plugin, "");
  assertEquals(plugin.commands.at(-1), "CloseSelf");
  assert(!JSON.stringify(plugin.commands).includes("update-ref"));
});
for (const field of ["head", "branch"]) {
  test(`agent-exit refuses cleanup when the confirmed ${field} changes`, async () => {
    const plugin = await checkout();
    await confirmDeletion(plugin);
    await reply(
      plugin,
      field === "head"
        ? listing().replaceAll(head, "1".repeat(40))
        : listing().replace("refs/heads/topic", "refs/heads/other"),
    );
    assert((await plugin.render()).includes("identity changed"));
    assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
  });
}
test("agent-exit refreshes a changed checkout before confirming its exact revision", async () => {
  const plugin = await checkout();
  const next = "1".repeat(40);
  const changed = listing()
    .replaceAll(head, next)
    .replace("refs/heads/topic", "refs/heads/rebased");
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assert((await plugin.render()).includes("Checking current checkout"));
  const count = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  await reply(plugin, changed);
  const confirmation = await plugin.render();
  assert(confirmation.includes(next));
  assert(confirmation.includes("Branch: rebased"));
  assert(confirmation.includes("commits merged into the local base branch"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, "", 128, "origin unavailable");
  await reply(plugin, changed);
  await cleanCheckout(plugin);
  await reply(plugin, "");
  await reply(plugin, "");
  assert(JSON.stringify(plugin.commands).includes(`"refs/heads/rebased","${next}"`));
  assertEquals(plugin.commands.at(-1), "CloseSelf");
});
test("agent-exit requires the complete confirmation to be visible before deleting", async () => {
  const plugin = await checkout();
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  await reply(plugin, listing());
  let count = plugin.commands.length;
  // A confirmation that has not rendered yet cannot be accepted by a repeated key.
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  for (const [rows, cols] of [
    [1, 1],
    [2, 8],
    [8, 24],
  ]) {
    await plugin.render(rows, cols);
    await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
    assertEquals(plugin.commands.length, count);
  }
  assert((await plugin.render(8, 24)).includes("Deletion disabled"));
  assert((await plugin.render(8, 24)).includes("Esc: cancel"));
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
  await plugin.render(24, 80);
  await plugin.event({ Key: { bare_key: "Delete", key_modifiers: [] } });
  assert(plugin.commands.length > count);
  count = plugin.commands.length;
  await plugin.event({ Key: { bare_key: "Enter", key_modifiers: [] } });
  assertEquals(plugin.commands.length, count);
});
test("agent-exit never inspects an SSH pane's path locally", async () => {
  const plugin = await loadPlugin("./agent-exit.ts", {
    configuration: { agent_cwd: "/repo/topic", agent_target: "ssh" },
  });
  assert(!(await plugin.render()).includes("Delete worktree"));
  assert(!JSON.stringify(plugin.commands).includes("RunCommand"));
});

for (const secondary of [false, true]) {
  test(`agent-exit Esc closes panes in a ${secondary ? "secondary" : "primary"} checkout`, async () => {
    const directory = secondary ? "/repo/topic" : "/repo";
    const plugin = await loadPlugin("./agent-exit.ts", {
      configuration: {
        agent_cwd: directory,
        agent_target: "local",
        agent_pane_id: "7",
      },
      replies: {
        GetPluginIds: { plugin_id: 1 },
        GetPaneInfo: { is_suppressed: false },
        ProjectConfigRead: { config: { default_branch: "main" } },
        RunCommand: null,
        MessageToPlugin: null,
        SetSelfMouseSelectionSupport: null,
        SubscribePaneOutput: null,
        SetPaneSubtitle: null,
        CloseTerminalPane: null,
        CloseSelf: null,
      },
    });
    await reply(plugin, directory + "\n");
    await reply(plugin, listing(secondary));
    assert((await plugin.render(24, 200)).includes("Esc: close pane"));
    if (secondary) {
      await prepareDeletion(plugin);
      await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
      assert(!plugin.commands.includes("CloseSelf"));
    }
    const count = plugin.commands.length;
    await plugin.event({ Key: { bare_key: "Esc", key_modifiers: [] } });
    assertEquals(plugin.commands.slice(count), [{ CloseTerminalPane: 7 }, "CloseSelf"]);
    assert(!JSON.stringify(plugin.commands).includes('"worktree","remove"'));
    assert(!JSON.stringify(plugin.commands).includes('"update-ref"'));
  });
}
