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
  await session.waitFor("fallback pane closes", () => !session.screen.includes("restart agent"));
  assert(!session.screen.includes("restart agent"));
});

const gitFixture = `#!/bin/sh
set -eu
git=/usr/bin/git
if [ ! -d "$HOME/repo/.git" ]; then
  "$git" init -q "$HOME/repo"
  "$git" -C "$HOME/repo" config user.name fixture
  "$git" -C "$HOME/repo" config user.email fixture@example.test
  printf base > "$HOME/repo/file"
  "$git" -C "$HOME/repo" add file
  "$git" -C "$HOME/repo" commit -qm base
  "$git" -C "$HOME/repo" worktree add -qb topic "$HOME/topic"
  printf unmerged > "$HOME/topic/file"
  "$git" -C "$HOME/topic" commit -qam topic
  printf dirty > "$HOME/topic/dirty"
fi
if [ "\${1:-}" = "-C" ] && [ "\${3:-}" = "worktree" ] && [ "\${4:-}" = "remove" ]; then
  [ ! -e "$HOME/require-vm-close" ] || [ -e "$HOME/vm-closed" ]
fi
exec "$git" "$@"
`;
const firecrackerFixture = `#!/bin/sh
set -eu
if [ "$1" = firecracker ] && [ "$2" = list ]; then
  printf '{"vms":[{"session":"work","name":"review","state":"running","vm_id":"/vm/review.socket","worktree":"%s/topic","metrics":{"generation":"42:100"}}],"recoveries":[]}' "$HOME"
elif [ "$1" = firecracker ] && [ "$2" = close ] && [ "$3" = review ] && [ "$4" = --session ] && [ "$5" = work ] && [ "$6" = --expected-identity ]; then
  printf '%s' "$7" | grep -q '"generation":"42:100"'
  [ "$8" = --expected-worktree ] && [ "$9" = "$HOME/topic" ]
  [ "$10" = --expected-head ] && [ -n "$11" ]
  [ "$12" = --expected-branch ] && [ "$13" = topic ]
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
  "agent-exit confirms before removing a dirty unmerged secondary worktree",
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
    await session.waitForText("unmerged commits");
    session.key("Delete");
    await session.waitFor(
      "confirmed deletion closes the exit view",
      () => !session.screen.includes("Delete worktree"),
    );
    assert(!session.screen.includes("Deletion failed"));
  },
);

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
      () => !session.screen.includes("Delete worktree"),
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
      () => !session.screen.includes("Delete again") && !session.screen.includes("Delete worktree"),
    );
    assert(!session.screen.includes("Deletion failed"));
  },
);
