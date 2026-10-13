#!/usr/bin/env node
import assert from "node:assert/strict"
import childProcess from "node:child_process"
import { EventEmitter } from "node:events"
import { syncBuiltinESMExports } from "node:module"
import { pathToFileURL } from "node:url"
import { readFileSync } from "node:fs"

// Run against the generated module, replacing only its child-process transport.
const children = []
let failure
childProcess.spawn = (binary, args, options) => {
  assert.equal(binary, "/test/yinz")
  assert.deepEqual(args, process.env.YINZ_REMOTE_PANE_TOKEN
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
const exports = await import(pathToFileURL(process.argv[2]))
assert.deepEqual(Object.keys(exports), ["Agent"], "only the plugin initializer is public")
const { Agent } = exports
// Expose the pure adapter only in the test module, never to OpenCode's plugin loader.
const instrumented = readFileSync(process.argv[2], "utf8") + "\nexport { toHookEvent };"
const { toHookEvent } = await import(`data:text/javascript;base64,${Buffer.from(instrumented).toString("base64")}`)
const lookups = []
let lookupFailure
let releaseLookup
const hooks = await Agent({ directory: "/work", client: { session: {
  get: async ({ path: { id } }) => {
    lookups.push(id)
    if (id === "delayed") await new Promise(resolve => { releaseLookup = resolve })
    if (lookupFailure === "throw") {
      lookupFailure = undefined
      throw new Error("lookup failed")
    }
    if (lookupFailure === "error") {
      lookupFailure = undefined
      return { error: "not found" }
    }
    return { data: { id, ...(id === "resumed-child" ? { parentID: "one" } : {}) } }
  },
} } })
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
assert.equal(children[1].payload.tool_input.command, "original", "snapshot at enqueue")
await close(children[1])
assert.equal(children[2].payload.hook_event_name, "PermissionRequest")
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
  assert.equal(next.payload.session_id, `after-${mode}`)
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
assert.deepEqual(children.slice(baseline).map(c => c.payload.delta),
  ["Hello world", "Other session", "Other message"])

for (const ending of ["session.error", "session.idle"]) {
  for (const parts of [[{ type: "text", text: "next turn" }], [{ type: "file", url: "file:///image" }]]) {
    await part("one", "success", "a", "Successful answer")
    await completed("one", "success")
    await event("session.idle", { sessionID: "one" })
    await drain()
    assert.equal(children.at(-1).payload.last_assistant_message, "Successful answer")
    await part("one", "pending", "a", "Stale pending text")
    const beforePrompt = children.length
    await prompt("one", parts)
    await drain()
    assert.equal(children.length, beforePrompt + 1)
    assert.equal(children.at(-1).payload.hook_event_name, "UserPromptSubmit")
    assert.equal(children.at(-1).payload.prompt,
      parts[0].type === "text" ? "next turn" : undefined)
    assert.equal(Object.hasOwn(children.at(-1).payload, "prompt"), parts[0].type === "text")
    await completed("one", "pending")
    await event(ending, { sessionID: "one" })
    await drain()
    assert.equal(children.at(-1).payload.hook_event_name, ending === "session.idle" ? "Stop" : "StopFailure")
    assert.equal(children.at(-1).payload.last_assistant_message, undefined)
    assert.equal(children.at(-1).payload.turn_id, undefined)
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
await drain()
const beforeResumed = children.length
await prompt("resumed-child", [])
await hooks["tool.execute.before"]({ sessionID: "resumed-child" }, { args: {} })
await hooks["tool.execute.after"]({ sessionID: "resumed-child" })
for (const type of ["permission.asked", "permission.updated", "permission.replied",
  "session.idle", "session.error", "session.compacted"]) {
  await event(type, { sessionID: "resumed-child" })
}
await part("resumed-child", "answer", "a", "hidden")
await completed("resumed-child", "answer")
await drain()
assert.equal(children.length, beforeResumed + 2)
assert.ok(children.slice(beforeResumed).every(c => c.payload.archive_only && c.payload.agent_id === "resumed-child" && c.payload.session_id === "one"))
assert.equal(lookups.filter(id => id === "resumed-child").length, 1)
await event("session.deleted", { info: { id: "resumed-child", parentID: "one" } })
await event("session.deleted", { info: { id: "unseen-child", parentID: "one" } })
await drain()
assert.equal(children.length, beforeResumed + 2)
assert.ok(children.slice(beforeResumed).every(c => c.payload.archive_only && c.payload.agent_id === "resumed-child" && c.payload.session_id === "one"))

for (const mode of ["throw", "error"]) {
  const id = `retry-${mode}`
  const count = children.length
  lookupFailure = mode
  await prompt(id, [])
  await drain()
  assert.equal(children.length, count)
  await prompt(id, [])
  await event("session.idle", { sessionID: id })
  await drain()
  assert.deepEqual(children.slice(count).map(c => c.payload.hook_event_name), ["UserPromptSubmit", "Stop"])
  assert.equal(lookups.filter(value => value === id).length, 2)
}

const beforeDelayed = children.length
const delayedArgs = { command: "original" }
await hooks["tool.execute.before"]({ sessionID: "delayed" }, { args: delayedArgs })
delayedArgs.command = "changed"
await event("session.idle", { sessionID: "one" })
await flush()
assert.equal(children.length, beforeDelayed, "callbacks return without waiting for metadata")
releaseLookup()
await drain()
assert.deepEqual(children.slice(beforeDelayed).map(c => c.payload.hook_event_name), ["PreToolUse", "Stop"])
assert.equal(children[beforeDelayed].payload.tool_input.command, "original")

const beforeChildTool = children.length
await hooks["tool.execute.before"]({ sessionID: "resumed-child", callID: "child-call", tool: "bash" }, { args: { command: "true" } })
await hooks["tool.execute.after"]({ sessionID: "resumed-child", callID: "child-call", tool: "bash" }, { output: "retained result" })
await event("message.part.updated", { part: { type: "tool", sessionID: "resumed-child", callID: "failed-call", tool: "bash", state: { status: "error", input: {}, error: "failed" } } })
await drain()
const childReports = children.slice(beforeChildTool).map(c => c.payload)
assert.equal(childReports.length, 3)
assert.ok(childReports.every(p => p.session_id === "one" && p.agent_id === "resumed-child" && p.archive_only))
assert.equal(childReports[1].tool_response, "retained result")
assert.equal(childReports[2].hook_event_name, "PostToolUseFailure")

console.log("OpenCode plugin regressions passed")

// The provider's native adapter emits the same contract as any other agent plugin.
for (const [type, expected] of Object.entries({
  "session.created": "SessionStart", "session.deleted": "SessionEnd",
  "session.idle": "Stop", "session.error": "StopFailure", "session.compacted": "PostCompact",
  "chat.message": "UserPromptSubmit", "tool.execute.before": "PreToolUse",
  "tool.execute.after": "PostToolUse", "permission.replied": "PostToolUse",
  "assistant.message": "MessageDisplay", "future.event": "future.event",
})) {
  const hook = toHookEvent({ type, properties: { sessionID: "session", cwd: "/work" } })
  assert.equal(hook.hook_event_name, expected)
  assert.equal(hook.session_id, "session")
  assert.equal(hook.cwd, "/work")
}
for (const [permission, metadata, patterns, expected] of [
  ["bash", { command: "cargo test", extra: true }, ["ignored"], { command: "cargo test", extra: true }],
  ["edit", { filePath: "/work/main.rs" }, ["ignored"], { file_path: "/work/main.rs" }],
  ["bash", {}, ["git status", "git diff"], { command: "git status git diff" }],
  ["read", { unrelated: true }, ["/work/main.rs"], { file_path: "/work/main.rs" }],
]) {
  const hook = toHookEvent({ type: "permission.asked", properties: {
    permission, metadata, patterns, tool: { callID: "call" }, sessionID: "one",
  } })
  assert.equal(hook.hook_event_name, "PermissionRequest")
  assert.equal(hook.tool_use_id, "call")
  assert.deepEqual(hook.tool_input, expected)
}
assert.equal(toHookEvent({ type: "tool.execute.before", properties: { tool: "question" } }).tool_name,
  "AskUserQuestion")
assert.equal(toHookEvent({ type: "assistant.message", properties: { text: "answer" } }).final_batch, true)
