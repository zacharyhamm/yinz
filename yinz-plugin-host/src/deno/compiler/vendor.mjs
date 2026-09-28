// Refresh the vendored compiler with: node compiler/vendor.mjs
// No npm install, global TypeScript, or network is needed by the checker itself.
import { createHash } from "node:crypto";
import { writeFile } from "node:fs/promises";

const version = "5.9.3";
const root = new URL("./", import.meta.url);
const base = `https://unpkg.com/typescript@${version}/`;
const hashes = {};
async function download(name) {
  const response = await fetch(base + name);
  if (!response.ok) throw new Error(`${name}: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  hashes[name] = createHash("sha256").update(bytes).digest("hex");
  return bytes.toString("utf8");
}
const declarations = {};
async function declaration(name) {
  if (Object.hasOwn(declarations, name)) return;
  const source = await download(`lib/${name}`);
  declarations[name] = source;
  for (const match of source.matchAll(/<reference lib="([^"]+)"/g)) {
    await declaration(`lib.${match[1]}.d.ts`);
  }
}
await declaration("lib.es2022.d.ts");
for (const [from, to] of [
  ["lib/typescript.js", "typescript.js"],
  ["LICENSE.txt", "LICENSE.txt"],
  ["ThirdPartyNoticeText.txt", "ThirdPartyNoticeText.txt"],
]) {
  await writeFile(new URL(to, root), await download(from));
}
await writeFile(new URL("libs.json", root), JSON.stringify(declarations));
await writeFile(new URL("provenance.json", root), JSON.stringify({
  package: "typescript", version,
  source: "https://github.com/microsoft/TypeScript/tree/v5.9.3",
  distribution: "https://registry.npmjs.org/typescript/-/typescript-5.9.3.tgz",
  mirror: base,
  license: "Apache-2.0",
  files: hashes,
}, null, 2) + "\n");
