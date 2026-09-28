import { test, session, assert } from "yinz:testing";
test("agent-policy receives the real session-start hook headlessly", async () => {
  session.start({ hooks: { session_start: "yinz:agent-policy" } });
  await session.waitFor("session hook in plugin log", () =>
    session.logs.includes("hook session_start"),
  );
  assert(session.logs.includes("session_start"));
});
