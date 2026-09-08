// Fast standalone compiler-host check: node unit/compiler.mjs
// Rust tests exercise the real restricted filesystem ops and JsRuntime.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { createHash } from "node:crypto";
const base = new URL("../", import.meta.url);
const read = name => readFileSync(new URL(name, base), "utf8");
const provenance = JSON.parse(read("compiler/provenance.json"));
const libs = JSON.parse(read("compiler/libs.json"));
assert.equal(provenance.version, "5.9.3");
for (const [name, expected] of Object.entries(provenance.files)) {
  const source = name.startsWith("lib/lib.") ? libs[name.slice(4)] :
    read("compiler/" + name.replace(/^lib\//, ""));
  assert.equal(createHash("sha256").update(source).digest("hex"), expected, name);
}
for (const source of Object.values(libs)) {
  for (const match of source.matchAll(/<reference lib="([^"]+)"/g)) {
    assert(Object.hasOwn(libs, `lib.${match[1]}.d.ts`));
  }
}
const files = new Map();
const context = vm.createContext({
  Deno: { core: { ops: {
    op_zellij_check_read(name) {
      assert(files.has(name), `unexpected read: ${name}`);
      return files.get(name);
    },
    op_zellij_check_resolve(name, referrer) {
      if (name === "zellij:plugin") return "/$zellij/sdk.d.ts";
      assert(name.startsWith("./"), `unsupported import: ${name}`);
      const target = new URL(name, "file://" + referrer).pathname;
      assert(files.has(target), `missing module: ${target}`);
      return target;
    },
  } } },
  __zellijLibs: libs,
});
vm.runInContext(read("compiler/typescript.js"), context);
vm.runInContext(read("compiler/host.js"), context);
function check(source, extension = "ts") {
  const entry = `/plugin/main.${extension}`;
  files.set(entry, source);
  context.input = { entry, sdk: read("sdk.d.ts") };
  vm.runInContext("zellijCheck(input)", context);
}
files.set("/plugin/dep.mts", "export const n: number = 42;");
files.set("/plugin/types.d.ts", "export interface Value { n: number }");
check(`import {n} from './dep.mts';
  import type {Value} from './types.d.ts';
  import zellij, {definePlugin} from 'zellij:plugin';
  const value: Value = {n};
  const last: number | undefined = [value.n].at(-1);
  const map = new Map<string, number>();
  await Promise.resolve(map);
  export default definePlugin({render: () => 'ok'});
  throw new Error('PLUGIN EXECUTED');`);
assert.throws(() => check("const n: number = 'wrong';"), /TS2322/);
assert.throws(() => check("import {definePlugin} from 'zellij:plugin'; definePlugin({render: () => 3});"), /TS2322/);
assert.throws(() => check("import zellij from 'zellij:plugin'; zellij.subscribee(['PaneUpdate']);"), /subscribee/);
assert.throws(() => check("import type {EventType} from 'zellij:plugin'; const event: EventType = 'PaneUpdat';"), /PaneUpdat/);
assert.throws(() => check("document.title = 'x';"), /Cannot find name 'document'/);
assert.throws(() => check("[1].toSorted();"), /toSorted/);
assert.throws(() => check("import 'react';"), /unsupported import/);
assert.throws(() => check("await import('https://example.com/a.ts');"), /unsupported import/);
assert.throws(() => check("export * from 'react';"), /unsupported import/);
assert.throws(() => check("/// <reference lib=\"dom\" />"), /lib.dom.d.ts.*not found/);
assert.throws(() => check("/// <reference types=\"node\" />"), /package type references/);
assert.throws(() => check("const = ;"), /TS\d+/);
assert.throws(() => check("/** @type {number} */ const n = 'wrong';", "js"), /TS2322/);
check("export declare const n: number;", "d.ts");
console.log("Offline compiler host checks passed");
