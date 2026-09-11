#!/usr/bin/env node
import assert from "node:assert/strict"
import childProcess from "node:child_process"
import { EventEmitter } from "node:events"
import { syncBuiltinESMExports } from "node:module"
import { pathToFileURL } from "node:url"

// Run against the generated module, replacing only its child-process transport.
const children = []
let failure
childProcess.spawn = (binary, args, options) => {
  assert.equal(binary, "/test/zellij")
  assert.deepEqual(args, process.env.ZELLIJ_REMOTE_PANE_TOKEN
    ? ["--pty-broker", "report", "opencode"]
    : ["action", "agent-report", "--hook-json", "--kind", "opencode"])
  assert.deepEqual(options.stdio, ["pipe", "ignore", "ignore"])
  if (failure === "spawn") {
    failure = undefined
    throw new Error("spawn failed")
  }
  const child = new EventEmitter()
  child.stdin = new EventEmitter()
  child.stdin.end = (text) => {
    if (failure === "stdin") {
      failure = undefined
      throw new Error("stdin failed")
    }
    child.payload = JSON.parse(text)
  }
  child.kill = () => { child.killed = true }
  children.push(child)
  return child
}
syncBuiltinESMExports()
const { Hocus } = await import(pathToFileURL(process.argv[2]))
const hooks = await Hocus({ directory: "/work" })
const event = (type, properties = {}) => hooks.event({ event: { type, properties } })
// One event-loop barrier drains Promise continuations; no child completes implicitly.
const flush = () => new Promise(setImmediate)
const close = async (child, code = 0) => {
  child.emit("close", code)
  await flush()
}
const start = (id) => event("session.created", { info: { id } })

await start("first")
const mutable = { command: "original" }
await hooks["tool.execute.before"]({ sessionID: "first", tool: "bash" }, { args: mutable })
mutable.command = "changed"
await event("permission.updated", { sessionID: "first", type: "bash", title: "echo hi" })
await flush()
assert.equal(children.length, 1, "callbacks return while the first report remains open")
await close(children[0])
assert.equal(children.length, 2)
assert.equal(children[1].payload.properties.args.command, "original", "snapshot at enqueue")
await close(children[1])
assert.equal(children[2].payload.type, "permission.updated")
await close(children[2])

// Serialization, synchronous spawn/stdin, asynchronous child/stdin and exit failures
// must never reject callbacks or poison later reports.
const circular = {}; circular.self = circular
await hooks["tool.execute.before"]({ sessionID: "first" }, { args: circular })
for (const mode of ["spawn", "stdin", "child-error", "stdin-error", "exit"]) {
  const count = children.length
  failure = mode
  await start(mode)
  await start(`after-${mode}`)
  await flush()
  if (mode !== "spawn") {
    const child = children[count]
    if (mode === "stdin") assert.equal(child.killed, true)
    if (mode === "child-error") child.emit("error", new Error("spawn error"))
    if (mode === "stdin-error") child.stdin.emit("error", new Error("EPIPE"))
    await flush()
    assert.equal(children.length, count + 1, "errors must still wait for close")
    await close(child, 1)
  }
  const next = children.at(-1)
  assert.equal(next.payload.properties.sessionID, `after-${mode}`)
  await close(next)
}

const part = (sessionID, messageID, id, text) => event("message.part.updated", {
  part: { sessionID, messageID, id, text, type: "text" },
})
const completed = (sessionID, id) => event("message.updated", {
  info: { sessionID, id, role: "assistant", time: { completed: 1 } },
})
const prompt = (sessionID, parts) => hooks["chat.message"]({ sessionID }, { parts })
const drain = async () => {
  let index = drained
  await flush()
  while (index < children.length) await close(children[index++])
  drained = index
}
let drained = children.length
const baseline = drained
await part("one", "message", "a", "old")
await part("one", "message", "b", " world")
await part("one", "message", "a", "Hello")
await part("two", "message", "a", "Other session")
await part("one", "other", "a", "Other message")
await completed("one", "message")
await completed("one", "message") // consumed parts cannot report twice
await completed("two", "message")
await completed("one", "other")
await drain()
assert.deepEqual(children.slice(baseline).map(c => c.payload.properties.text),
  ["Hello world", "Other session", "Other message"])

for (const ending of ["session.error", "session.idle"]) {
  for (const parts of [[{ type: "text", text: "next turn" }], [{ type: "file", url: "file:///image" }]]) {
    await part("one", "success", "a", "Successful answer")
    await completed("one", "success")
    await event("session.idle", { sessionID: "one" })
    await drain()
    assert.equal(children.at(-1).payload.properties.lastAssistantMessage, "Successful answer")
    await part("one", "pending", "a", "Stale pending text")
    await prompt("one", parts)
    await completed("one", "pending")
    await event(ending, { sessionID: "one" })
    await drain()
    assert.equal(children.at(-1).payload.type, ending)
    assert.equal(children.at(-1).payload.properties.lastAssistantMessage, undefined)
    assert.equal(children.at(-1).payload.properties.turnID, undefined)
  }
}
const beforeChild = children.length
await event("session.created", { info: { id: "child", parentID: "one" } })
await prompt("child", [{ type: "text", text: "hidden" }])
await event("permission.asked", { sessionID: "child" })
await part("child", "message", "a", "hidden")
await completed("child", "message")
await event("session.idle", { sessionID: "child" })
await event("session.deleted", { info: { id: "child" } })
await flush()
assert.equal(children.length, beforeChild)
console.log("OpenCode plugin regressions passed")
