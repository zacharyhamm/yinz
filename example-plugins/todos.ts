// Read Agent's shared todo tree. Agent owns its schema and mutation invariants.
import z, { definePlugin, type DataRecord } from "yinz:plugin";
const namespace = "shared:yinz";
const D = z.data;
let items: DataRecord[] = [],
  name = "",
  status = "asking for permission";
export default definePlugin({
  async load() {
    try {
      await z.waitForPermissions(["PluginData", "SharedPluginData"]);
      const entities = D.catalog(namespace);
      if (entities.length === 0) throw new Error("no agent data in this session");
      for (const entity of entities)
        z.log(
          `${namespace}.${entity.def.name}: ${entity.def.attrs.map((a) => `${a.name} ${a.kind}`).join(", ")}`,
        );
      const cwd = z.getPluginIds().initial_cwd;
      const project = D.fetch({ namespace, entity: "project" })
        .records.filter(
          (p) =>
            typeof p["root"] === "string" &&
            p["root"] !== "" &&
            (cwd === p["root"] || cwd.startsWith(p["root"] + "/")),
        )
        .sort((a, b) => String(b["root"]).length - String(a["root"]).length)[0];
      if (!project) throw new Error(`no agent project covers ${cwd}`);
      name = String(project["name"]);
      const watch = D.watch(
        {
          namespace,
          entity: "todo",
          filter: D.eq("root", project["root"] ?? null),
          sort: [["id", "Asc"]],
          limit: 500,
        },
        (snapshot) => {
          items = snapshot.records;
          return true;
        },
      );
      items = watch.initial.records;
      status = "";
    } catch (error) {
      status = String(error);
    }
    z.requestRender();
  },
  render(rows, cols) {
    if (status) return z.ui.text(status).dim().render({ width: cols, height: 1 });
    const list = z.ui.nestedList(),
      seen = new Set<string>();
    function walk(parent: unknown, depth: number): void {
      // oxlint-disable-next-line eslint/no-shadow -- The nested callback has its own independent operation state.
      for (const item of items.filter((item) => (item["parent_id"] ?? null) === parent)) {
        if (seen.has(String(item["id"]))) continue;
        seen.add(String(item["id"]));
        const text = z.ui.text(
          `${item["done"] === true ? "x" : "-"} ${String(item["text"] ?? "")}`,
        );
        list.item(item["done"] === true ? text.dim() : text, depth);
        walk(item["id"], depth + 1);
      }
    }
    walk(null, 0);
    if (items.length === 0) list.item(z.ui.text("nothing on the list").dim());
    return (
      z.ui.text(`${name} todos`).emphasis(0).render() +
      "\n" +
      list.render({ y: 1, width: cols, height: Math.max(0, rows - 1) })
    );
  },
});
