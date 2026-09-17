import { test, session, assert } from "zellij:testing";
test("agent-exit visible fallback handles keys, resize and reload", async () => {
  session.start();
  session.loadPlugin("zellij:agent-exit");
  await session.waitForText("restart agent");
  session.resize(30, 100);
  await session.waitForText("restart agent");
  session.reload("zellij:agent-exit");
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
    session.loadPlugin("zellij:agent-exit", {
      configuration: { agent_cwd: `${root}/repo`, agent_target: "local" },
    });
    await session.waitForText("Agent exited.");
    assert(!session.screen.includes("delete worktree"));
  },
);

test(
  "agent-exit does not offer deletion outside a Git checkout",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture({}, { git: "#!/bin/sh\nexit 128\n" });
    session.start();
    session.loadPlugin("zellij:agent-exit", {
      configuration: { agent_cwd: root, agent_target: "local" },
    });
    await session.waitForText("No usable Git worktree");
    assert(!session.screen.includes("delete worktree"));
  },
);

test(
  "agent-exit shows malformed project configuration without inspecting the checkout",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture(
      { "repo/.zellij/swamp-workflows.kdl": "broken {}" },
      { git: gitFixture },
    );
    session.start();
    session.loadPlugin("zellij:agent-exit", {
      configuration: { agent_cwd: `${root}/topic`, agent_target: "local" },
    });
    await session.waitForText("must contain only one agent_exit block");
    assert(!session.screen.includes("delete worktree"));
  },
);

test(
  "agent-exit confirms before removing a dirty unmerged secondary worktree",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const root = session.fixture({}, { git: gitFixture });
    session.start();
    session.loadPlugin("zellij:agent-exit", {
      configuration: { agent_cwd: `${root}/topic`, agent_target: "local" },
    });
    await session.waitForText("Delete: delete worktree and branch");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.key("Delete");
    await session.waitFor(
      "confirmed deletion closes the exit view",
      () => !session.screen.includes("delete worktree"),
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
        "zellij-fixture": firecrackerFixture,
      },
    );
    session.start();
    session.loadPlugin("zellij:agent-exit", {
      configuration: {
        agent_target: "firecracker",
        agent_firecracker_worktree: `${root}/topic`,
        agent_firecracker_vm: "review",
        agent_session: "work",
        agent_zellij_executable: "zellij-fixture",
      },
    });
    await session.waitForText("Delete: delete worktree and branch");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.key("Delete");
    await session.waitFor(
      "VM close permits Git deletion",
      () => !session.screen.includes("delete worktree"),
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
      { git: gitFixture, "zellij-fixture": firecrackerFixture },
    );
    session.start();
    session.loadPlugin("zellij:agent-exit", {
      configuration: {
        agent_target: "firecracker",
        agent_firecracker_worktree: `${root}/topic`,
        agent_firecracker_vm: "review",
        agent_session: "work",
        agent_zellij_executable: "zellij-fixture",
      },
    });
    await session.waitForText("Delete: delete worktree and branch");
    session.key("Delete");
    await session.waitForText("Delete again");
    session.key("Delete");
    await session.waitForText("Worktree is still leased by");
    assert(session.screen.includes("Delete: delete worktree and branch"));
  },
);

import { failedWorkflow, deliveryValues } from "./testing-fixtures/workflow.ts";
test(
  "agent-exit attaches to the real bridge and renders a persisted launch failure",
  { grants: ["ReadFiles", "RunCommands"] },
  async () => {
    const directory = failedWorkflow();
    session.start({ backgroundPlugins: ["zellij:swamp-bridge"] });
    session.loadPlugin("zellij:agent-exit", {
      configuration: { agent_cwd: directory, agent_exit_token: "fixture-launch" },
    });
    await session.waitForText("failed");
    await session.waitFor("launch error persisted", () =>
      deliveryValues().some(
        (value) => value.outcome === "failed" && String(value.error).includes("Swamp"),
      ),
    );
    await session.waitForText("fixture-launch");
    session.resize(30, 100);
    session.key("Home");
    await session.waitForText("failed");
  },
);
