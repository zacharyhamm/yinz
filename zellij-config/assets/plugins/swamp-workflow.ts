import { definePlugin } from "zellij:plugin";
import { view } from "zellij:workflow-view";
export default definePlugin({
  load: () => {
    view.load();
  },
  update: view.update,
  pipe: view.pipe,
  render: view.render,
});
