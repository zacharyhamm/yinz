import { test, session, assert } from "yinz:testing";
test("agent-exit visible fallback handles keys, resize and reload", async () => {
  session.start();
  session.loadPlugin("yinz:agent-exit");
  await session.waitForText("restart agent");
  session.resize(30, 100);
  await session.waitForText("restart agent");
  session.reload("yinz:agent-exit");
  await session.waitForText("restart agent");
  session.key("Esc");
  await session.waitFor(
    "fallback pane closes",
    () => !session.panes.some((pane) => pane.plugin_url === "yinz:agent-exit"),
  );
  assert(!session.screen.includes("restart agent"));
});

const gitFixture = `#!/bin/sh
set -eu
git=/usr/bin/git
if [ ! -d "$HOME/repo/.git" ]; then
  "$git" init -qb main "$HOME/repo"
  "$git" -C "$HOME/repo" config user.name fixture
  "$git" -C "$HOME/repo" config user.email fixture@example.test
  printf base > "$HOME/repo/file"
  "$git" -C "$HOME/repo" add file
  "$git" -C "$HOME/repo" commit -qm base
  "$git" -C "$HOME/repo" worktree add -qb topic "$HOME/topic"
  printf unmerged > "$HOME/topic/file"
  "$git" -C "$HOME/topic" commit -qam topic
  if [ ! -e "$HOME/unmerged" ]; then
    "$git" -C "$HOME/repo" merge -q --ff-only topic
  fi
  base=main
  if [ -e "$HOME/trunk" ]; then
    "$git" -C "$HOME/repo" branch -m main trunk
    base=trunk
    mkdir -p "$HOME/repo/.yinz"
    printf 'agent_exit { default_branch "trunk"; }' > "$HOME/repo/.yinz/swamp-workflows.kdl"
  fi
  if [ -e "$HOME/remote" ]; then
    "$git" init --bare -q "$HOME/origin.git"
    "$git" -C "$HOME/repo" remote add origin "$HOME/origin.git"
    base_head=$("$git" -C "$HOME/repo" rev-parse topic^)
    "$git" -C "$HOME/origin.git" fetch -q "$HOME/repo" "$base_head:refs/heads/$base"
    "$git" -C "$HOME/repo" fetch -q origin
    if [ -e "$HOME/remote-merged" ]; then
      "$git" -C "$HOME/origin.git" fetch -q "$HOME/repo" "topic:refs/heads/$base"
    fi
    if [ -e "$HOME/fetch-fails" ]; then
      "$git" -C "$HOME/repo" update-ref "refs/remotes/origin/$base" topic
      "$git" -C "$HOME/repo" remote set-url origin "$HOME/missing.git"
    fi
  fi
  if [ -e "$HOME/missing-local" ]; then
    "$git" -C "$HOME/repo" checkout -q --detach
    "$git" -C "$HOME/repo" branch -q -D "$base"
  fi
  [ ! -e "$HOME/dirty-tracked" ] || printf dirty > "$HOME/topic/file"
  [ ! -e "$HOME/dirty-untracked" ] || printf dirty > "$HOME/topic/untracked"
  if [ -e "$HOME/dirty-staged" ]; then
    printf staged > "$HOME/topic/file"
    "$git" -C "$HOME/topic" add file
  fi
fi
if [ "\${1:-}" = "-C" ] && [ "\${3:-}" = "worktree" ] && [ "\${4:-}" = "remove" ]; then
  [ ! -e "$HOME/require-vm-close" ] || [ -e "$HOME/vm-closed" ]
fi
if [ "\${3:-}" = fetch ] && [ -e "$HOME/change-during-fetch" ]; then
  printf dirty > "$HOME/topic/during-fetch"
fi
exec "$git" "$@"
`;
const firecrackerFixture = `#!/bin/sh
set -eu
# Match the provider: VM worktree identities use physical local paths.
worktree=$(cd "$HOME/topic" && pwd -P)
if [ "$1" = firecracker ] && [ "$2" = list ]; then
  printf '{"vms":[{"session":"work","name":"review","state":"running","vm_id":"/vm/review.socket","worktree":"%s","metrics":{"generation":"42:100"}}],"recoveries":[]}' "$worktree"
elif [ "$1" = firecracker ] && [ "$2" = close ] && [ "$3" = review ] && [ "$4" = --session ] && [ "$5" = work ] && [ "$6" = --expected-identity ]; then
  printf '%s' "$7" | grep -q '"generation":"42:100"'
  [ "$8" = --expected-worktree ] && [ "$9" = "$worktree" ]
  [ "\${10}" = --expected-head ] && [ -n "\${11}" ]
  [ "\${12}" = --expected-branch ] && [ "\${13}" = topic ]
  if [ -e "$HOME/other-lease" ]; then
    echo 'Worktree is still leased by other' >&2
    exit 1
  fi
  /usr/bin/git -C "$HOME/repo" worktree remove --force -- "$HOME/topic"
  : > "$HOME/vm-closed"
else
  exit 9
fi
`;

test(
  "agent-exit does not offer deletion for a primary checkout",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture({}, { git: gitFixture });
    session.start();
    session.loadPlugin("yinz:agent-exit", {
      configuration: { agent_cwd: `${root}/repo`, agent_target: "local" },
    });
    await session.waitForText("Agent exited.");
    assert(!session.screen.includes("Delete worktree"));
  },
);

test(
  "agent-exit does not offer deletion outside a Git checkout",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture({}, { git: "#!/bin/sh\nexit 128\n" });
    session.start();
    session.loadPlugin("yinz:agent-exit", {
      configuration: { agent_cwd: root, agent_target: "local" },
    });
    await session.waitForText("No usable Git worktree");
    assert(!session.screen.includes("Delete worktree"));
  },
);

test(
  "agent-exit shows malformed project configuration while inspecting the checkout",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture(
      { "repo/.yinz/swamp-workflows.kdl": "broken {}" },
      { git: gitFixture },
    );
    session.start();
    session.loadPlugin("yinz:agent-exit", {
      configuration: { agent_cwd: `${root}/topic`, agent_target: "local" },
    });
    await session.waitForText("must contain only one agent_exit block");
    await session.waitForText("Delete worktree & branch and close pane");
  },
);

test(
  "agent-exit removes a clean secondary worktree merged into its base",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture({}, { git: gitFixture });
    session.start();
    session.loadPlugin("yinz:agent-exit", {
      configuration: { agent_cwd: `${root}/topic`, agent_target: "local" },
    });
    await session.waitForText("Delete: Delete worktree & branch and close pane");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.resize(8, 24);
    await session.waitForText("Deletion disabled");
    session.key("Delete");
    session.resize(30, 100);
    await session.waitForText("Delete again");
    await session.waitForText("commits merged into the local base branch");
    session.key("Delete");
    await session.waitFor(
      "confirmed deletion closes the exit view",
      () => !session.panes.some((pane) => pane.plugin_url === "yinz:agent-exit"),
    );
    await session.waitForText("yinz-test>");
    assert(!session.screen.includes("Deletion failed"));
    session.text(
      'test ! -d "$HOME/topic" && ! /usr/bin/git -C "$HOME/repo" show-ref --verify --quiet refs/heads/topic && printf "CLEANUP_%s\\n" OK',
    );
    session.key("Enter");
    await session.waitForText("CLEANUP_OK");
  },
);

for (const [name, markers, succeeds] of [
  ["remote ahead of local", ["unmerged", "remote-merged"], true],
  ["local ahead of remote", [], true],
  ["neither merged", ["unmerged"], false],
  ["failed fetch with local proof", ["fetch-fails"], true],
  ["failed fetch with stale remote proof", ["unmerged", "fetch-fails"], false],
  ["missing local branch", ["unmerged", "remote-merged", "missing-local"], true],
  ["configured trunk", ["unmerged", "remote-merged", "trunk"], true],
  ["changes during fetch", ["remote-merged", "change-during-fetch"], false],
] as const) {
  test(`agent-exit fetch cleanup: ${name}`, { grants: ["ReadFiles", "RunCommands"] }, async () => {
    const files: Record<string, string> = { remote: "" };
    for (const marker of markers) files[marker] = "";
    const root = session.fixture(files, { git: gitFixture });
    session.start();
    session.loadPlugin("yinz:agent-exit", {
      configuration: { agent_cwd: `${root}/topic`, agent_target: "local" },
    });
    await session.waitForText("Delete: Delete worktree & branch and close pane");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.key("Delete");
    if (!succeeds) {
      await session.waitForText("Deletion failed");
      session.key("Esc");
    }
    await session.waitFor(
      "exit view closes",
      () => !session.panes.some((pane) => pane.plugin_url === "yinz:agent-exit"),
    );
    await session.waitForText("yinz-test>");
    session.text(
      succeeds
        ? 'test ! -d "$HOME/topic" && ! /usr/bin/git -C "$HOME/repo" show-ref --verify --quiet refs/heads/topic && printf "VERIFIED_%s\\n" OK'
        : 'test -d "$HOME/topic" && /usr/bin/git -C "$HOME/repo" show-ref --verify --quiet refs/heads/topic && printf "VERIFIED_%s\\n" OK',
    );
    session.key("Enter");
    await session.waitForText("VERIFIED_OK");
  });
}

for (const [marker, reason] of [
  ["unmerged", "merged into local main"],
  ["dirty-tracked", "uncommitted changes"],
  ["dirty-untracked", "uncommitted changes"],
  ["dirty-staged", "uncommitted changes"],
]) {
  test(
    `agent-exit preserves worktree and branch with ${marker}`,
    { grants: ["ReadFiles", "RunCommands"] },
    async () => {
      const root = session.fixture({ [marker]: "" }, { git: gitFixture });
      session.start();
      session.loadPlugin("yinz:agent-exit", {
        configuration: { agent_cwd: `${root}/topic`, agent_target: "local" },
      });
      await session.waitForText("Delete: Delete worktree & branch and close pane");
      session.key("Delete");
      await session.waitForText("Delete again");
      session.key("Delete");
      await session.waitForText(reason);
      assert(session.screen.replace(/\s+/gu, " ").includes("worktree and branch preserved"));
      session.key("Esc");
      await session.waitFor(
        "exit view closes",
        () => !session.panes.some((pane) => pane.plugin_url === "yinz:agent-exit"),
      );
      await session.waitForText("yinz-test>");
      session.text(
        'test -d "$HOME/topic" && /usr/bin/git -C "$HOME/repo" show-ref --verify --quiet refs/heads/topic && printf "PRESERVED_%s\\n" OK',
      );
      session.key("Enter");
      await session.waitForText("PRESERVED_OK");
    },
  );
}

test(
  "agent-exit closes a guarded Firecracker lease before removing its worktree",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture(
      { "require-vm-close": "" },
      {
        git: gitFixture,
        "yinz-fixture": firecrackerFixture,
      },
    );
    session.start();
    session.loadPlugin("yinz:agent-exit", {
      configuration: {
        agent_target: "firecracker",
        agent_firecracker_worktree: `${root}/topic`,
        agent_firecracker_vm: "review",
        agent_session: "work",
        agent_yinz_executable: "yinz-fixture",
      },
    });
    await session.waitForText("Delete: Delete worktree & branch and close pane");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.key("Delete");
    await session.waitFor(
      "VM close permits Git deletion",
      () => !session.panes.some((pane) => pane.plugin_url === "yinz:agent-exit"),
    );
    assert(!session.screen.includes("Deletion failed"));
  },
);

test(
  "agent-exit refuses Git deletion while another Firecracker lease remains",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture(
      { "other-lease": "", "require-vm-close": "" },
      { git: gitFixture, "yinz-fixture": firecrackerFixture },
    );
    session.start();
    session.loadPlugin("yinz:agent-exit", {
      configuration: {
        agent_target: "firecracker",
        agent_firecracker_worktree: `${root}/topic`,
        agent_firecracker_vm: "review",
        agent_session: "work",
        agent_yinz_executable: "yinz-fixture",
      },
    });
    await session.waitForText("Delete: Delete worktree & branch and close pane");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.key("Delete");
    await session.waitForText("Worktree is still leased by");
    assert(session.screen.includes("Delete: retry cleanup"));
  },
);

import { failedWorkflow, deliveryValues } from "./testing-fixtures/workflow.ts";
test(
  "agent-exit attaches to the real bridge and renders a persisted launch failure",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const directory = failedWorkflow();
    session.start({ backgroundPlugins: ["yinz:swamp-bridge"] });
    session.loadPlugin("yinz:agent-exit", {
      configuration: { agent_cwd: directory, agent_exit_token: "fixture-launch" },
    });
    await session.waitForText("failed");
    await session.waitFor("launch error persisted", () =>
      deliveryValues().some(
        (value) => value.outcome === "failed" && String(value.error).includes("Swamp"),
      ),
    );
    await session.waitForText("Details");
    const lines = session.screen.split("\n");
    const row = lines.findIndex((line) => line.includes("") && line.includes("Details"));
    const column = lines[row]!.indexOf("Details");
    for (const event_type of ["Press", "Release"] as const) {
      session.mouse({
        event_type,
        left: true,
        right: false,
        middle: false,
        wheel_up: false,
        wheel_down: false,
        shift: false,
        alt: false,
        ctrl: false,
        position: { line: row, column },
      });
    }
    await session.waitForText("Hide details");
    session.resize(50, 100);
    await session.waitForText("fixture-launch");
    session.resize(30, 100);
    session.key("Home");
    await session.waitForText("failed");
  },
);

test(
  "agent-exit deletes a secondary worktree after a configured launch fails",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture(
      { "repo/.yinz/swamp-workflows.kdl": 'agent_exit { workflow "fixture-exit"; }' },
      { git: gitFixture, swamp: "echo fixture-swamp-unavailable >&2; exit 1" },
    );
    session.start({ backgroundPlugins: ["yinz:swamp-bridge"] });
    session.loadPlugin("yinz:agent-exit", {
      configuration: {
        agent_cwd: `${root}/topic`,
        agent_target: "local",
        agent_exit_token: "cleanup-launch",
      },
    });
    await session.waitForText("failed");
    await session.waitForText("Delete worktree & branch and close pane");
    session.key("Delete");
    await session.waitForText("This cancels the workflow");
    session.key("Esc");
    await session.waitForText("Delete worktree & branch and close pane");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.key("Delete");
    await session.waitFor("cleanup readiness persists", () =>
      deliveryValues().some((entry) => entry.cleanup && entry.closed && !entry.cancel_pending),
    );
    await session.waitFor(
      "exit pane closes after cleanup",
      () => !session.panes.some((pane) => pane.plugin_url === "yinz:agent-exit"),
    );
    assert(!session.screen.includes("Deletion failed"));
  },
);
