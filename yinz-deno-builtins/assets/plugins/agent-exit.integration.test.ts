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
  await session.waitFor(
    "fallback pane closing render",
    () => !session.screen.includes("restart agent"),
  );
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
    printf 'agent_exit { default_branch "trunk"; }' > "$HOME/repo/.yinz/agent-exit.kdl"
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
if [ "\${3:-}" = fetch ] && [ -e "$HOME/change-during-fetch" ]; then
  printf dirty > "$HOME/topic/during-fetch"
fi
exec "$git" "$@"
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
    const root = session.fixture({ "repo/.yinz/agent-exit.kdl": "broken {}" }, { git: gitFixture });
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
