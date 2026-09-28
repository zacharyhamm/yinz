import { test, assert, assertEquals, renderText } from "yinz:testing";
import { view } from "yinz:workflow-view";
test("workflow-view renders its initial state at narrow and wide sizes", () => {
  assertEquals(view.status, "No run attached");
  assert(renderText(view.render(24, 80)).includes("No run attached"));
  assert(typeof view.render(1, 4) === "string");
  assertEquals(view.pending, []);
});
