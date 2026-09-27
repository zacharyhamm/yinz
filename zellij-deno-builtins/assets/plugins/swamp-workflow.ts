import { definePlugin } from "zellij:plugin";
import { view } from "./workflow-view.ts";
export default definePlugin({
  load: () => {
    view.load();
  },
  update: view.update,
  pipe: view.pipe,
  render: view.render,
});
