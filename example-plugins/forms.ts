import { definePlugin, yinz, type WidgetInteraction } from "yinz:plugin";

const ui = yinz.ui;
const form = ui.form([
  { kind: "textInput", id: "name", label: "Name", value: "日本語" },
  { kind: "checkbox", id: "frames", label: "Pane frames", value: true },
  { kind: "radioGroup", id: "mode", label: "Mode", options: ["Local", "Remote"] },
  { kind: "select", id: "layout", label: "Layout", options: ["Default", "Compact"] },
  { kind: "combo", id: "branch", label: "Branch", options: ["main", "feature"] },
  { kind: "button", id: "save", label: "Save" },
  { kind: "button", id: "unavailable", label: "Unavailable", disabled: true },
]);
const dialog = ui.dialog({
  title: "Review values",
  message: "Keep these values?",
  help: "Esc: cancel",
  fields: [
    { kind: "button", id: "confirm", label: "Keep" },
    { kind: "button", id: "cancel", label: "Cancel" },
  ],
});
const markdown = ui.markdown(
  "# Shared widgets\n\nUse **Tab** to move, **Enter** to edit, and **Esc** to cancel.\n\n- Selections start at zero.\n- 日本語 uses terminal columns.\n\n|Control|Purpose|\n|-|-|\n|Combo|Edit or choose|\n|Select|Choose an option|",
);
let confirming = false;
let status = "Edit the form";
let scroll = 0;
const active = (): import("yinz:plugin").FormWidget => (confirming ? dialog : form);
// oxlint-disable-next-line typescript/prefer-readonly-parameter-types -- This helper shares mutable plugin state and SDK wire types with its callers.
function interact(result: WidgetInteraction): boolean {
  if (result.kind === "buttonPressed") {
    if (confirming) {
      if (result.id === "confirm") status = JSON.stringify(form.values());
      confirming = false;
    } else if (result.id === "save") {
      dialog.setStatus("Values stay in this plugin.");
      confirming = true;
    }
    return true;
  }
  if (result.kind === "cancel" && confirming) {
    confirming = false;
    return true;
  }
  return result.shouldRender;
}
export default definePlugin({
  load() {
    yinz.subscribe(["Key", "Mouse", "PastedText"]);
    yinz.on("Key", (key) => {
      if (key.bare_key === "PageDown") {
        scroll += 1;
        return true;
      }
      if (key.bare_key === "PageUp") {
        scroll = Math.max(0, scroll - 1);
        return true;
      }
      return interact(active().handleKey(key));
    });
    yinz.on("Mouse", (mouse) => interact(active().handleMouse(mouse)));
    yinz.on("PastedText", (text) => interact(active().insertPastedText(text)));
  },
  render(rows, columns) {
    if (confirming) return dialog.render({ width: columns, height: rows });
    const formRows = Math.min(10, rows);
    let output = form.render({ width: columns, height: formRows });
    if (rows > formRows) {
      output += ui
        .text(yinz.text.truncate(status, columns))
        .dim()
        .render({ y: formRows, width: columns, height: 1 });
      output += markdown.render({
        y: formRows + 1,
        width: columns,
        height: Math.max(0, rows - formRows - 1),
        scroll,
      }).text;
    }
    return output;
  },
});
