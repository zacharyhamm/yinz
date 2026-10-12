import { test, session, assertEquals } from "yinz:testing";

test(
  "Mother Overview reports setup blockers and controls its supervisor",
  { grants: ["RunCommands"] },
  async () => {
    session.fixture(
      { ".claude/settings.json": "{}", ".claude.json": "{}" },
      {
        // Keep the agent on the fixture PATH instead of Bubblewrap's host-derived PATH.
        bwrap: "exit 1\n",
        claude: "printf 'MOTHER FIXTURE READY\\r\\n'\nwhile IFS= read -r line; do :; done\n",
      },
    );
    session.start({ columns: 180, rows: 36 });
    await session.waitForText("yinz-test>");
    session.key("Ctrl e");
    session.text("m");
    await session.waitForText("Launch mother");
    session.text("claude");
    await session.waitForText("> claude");
    session.key("Enter");
    await session.waitForText("MOTHER FIXTURE READY");
    session.action("NextPaneTab;");
    await session.waitForText("Supervision:");
    await session.waitForText("lifecycle hooks");
    const paused = await session.agent({ Mother: "Pause" });
    assertEquals(paused.outcome, "ok");
    await session.waitForText("Supervision: paused");
    session.key("p");
    await session.waitFor(
      "Overview resumed observation",
      () => !session.screen.includes("Supervision: paused"),
    );
    session.action("PreviousPaneTab;");
    await session.waitForText("MOTHER FIXTURE READY");
  },
);
