#!/usr/bin/env node
import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import fs from 'node:fs';
import { EventEmitter } from 'node:events';
import { syncBuiltinESMExports } from 'node:module';
import { pathToFileURL } from 'node:url';

const children = [];
let spawns = 0;
let failure;
childProcess.spawn = (binary, args, options) => {
  spawns++;
  assert.equal(binary, '/test/yinz');
  assert.deepEqual(args, process.env.YINZ_REMOTE_PANE_TOKEN
    ? ['--pty-broker', 'report', 'pi']
    : ['action', 'agent-report', '--hook-json', '--kind', 'pi']);
  assert.deepEqual(options.stdio, ['pipe', 'ignore', 'ignore']);
  if (failure === 'spawn') { failure = undefined; throw Error('spawn'); }
  const child = new EventEmitter();
  child.stdin = new EventEmitter();
  child.stdin.end = text => {
    if (failure === 'stdin') { failure = undefined; throw Error('stdin'); }
    child.payload = JSON.parse(text);
  };
  child.kill = signal => { assert.equal(signal, 'SIGKILL'); child.killed = true; };
  children.push(child);
  return child;
};
fs.existsSync = path => path === '/test/yinz';
syncBuiltinESMExports();
const factory = (await import(pathToFileURL(process.argv[2]))).default;
const flush = () => new Promise(setImmediate);
const load = () => {
  const handlers = new Map();
  factory({ on(name, handler) { handlers.set(name, handler); } });
  return handlers;
};
let session = 'one';
let idle = true;
const ctx = { mode: 'tui', cwd: '/work', isIdle: () => idle, sessionManager: { getSessionId: () => session } };
let handlers = load();
assert.equal(load().size, 0, 'installed and inline copies have one owner');
const emit = (name, event = {}) => handlers.get(name)(event, ctx);
let consumed = 0;
const next = async () => {
  await flush();
  const child = children[consumed++];
  assert.ok(child, 'expected report');
  child.emit('close', 0);
  await flush();
  return child.payload;
};
for (const mode of ['json', 'print', 'rpc']) {
  ctx.mode = mode;
  for (const [name, event] of [
    ['session_start', { reason: 'startup' }],
    ['before_agent_start', { prompt: 'headless' }],
    ['tool_execution_start', { toolName: 'bash', toolCallId: 'tool', args: { command: 'true' } }],
    ['tool_execution_end', { toolName: 'bash', toolCallId: 'tool', isError: false }],
    ['tool_execution_end', { toolName: 'bash', toolCallId: 'tool', isError: true }],
    ['ui_prompt_start', { title: 'Continue?', kind: 'confirm' }],
    ['ui_prompt_end', {}],
    ['session_before_compact', {}],
    ['session_compact', { reason: 'manual' }],
    ['session_compact_failed', { reason: 'manual' }],
    ['message_end', { message: { role: 'assistant', content: [{ type: 'text', text: 'done' }], stopReason: 'stop' } }],
    ['agent_settled', {}],
    ['session_shutdown', { reason: 'exit' }],
  ]) {
    await emit(name, event);
    await flush();
    assert.equal(spawns, 0, `${mode} ${name} must not spawn a pane reporter`);
  }
  handlers = load();
  assert.ok(handlers.size, `${mode} shutdown releases ownership`);
}
ctx.mode = 'tui';
// An earlier session_start handler can await a dialog before our startup handler runs.
emit('ui_prompt_start', { title: 'Startup confirmation?', kind: 'confirm' });
const startup = await next();
assert.equal(startup.hook_event_name, 'SessionStart');
assert.equal(startup.source, 'startup');
assert.equal(startup.session_id, 'one');
assert.equal(startup.turn_id, undefined);
assert.equal((await next()).hook_event_name, 'Elicitation');
emit('ui_prompt_end');
assert.equal((await next()).source, 'idle');
emit('ui_prompt_start', { title: 'Another startup confirmation?', kind: 'confirm' });
assert.equal((await next()).hook_event_name, 'Elicitation', 'announce early startup only once');
emit('ui_prompt_end');
await next();
emit('session_start', { reason: 'startup' });
assert.equal((await next()).source, 'startup');
// An earlier before_agent_start handler can show a dialog before our handler runs.
emit('ui_prompt_start', { title: 'Continue before starting?', kind: 'confirm' });
const earlyDialog = await next();
assert.equal(earlyDialog.hook_event_name, 'Elicitation');
assert.equal(earlyDialog.session_id, 'one');
assert.equal(earlyDialog.turn_id, undefined);
emit('ui_prompt_end');
const earlyResult = await next();
assert.equal(earlyResult.hook_event_name, 'ElicitationResult');
assert.equal(earlyResult.source, 'idle');
assert.equal(earlyResult.turn_id, undefined);
emit('before_agent_start', { prompt: 'review' });
const submit = await next();
assert.equal(submit.prompt, 'review');
assert.ok(submit.turn_id);
// Pi is still idle while later before_agent_start handlers await a dialog.
emit('ui_prompt_start', { title: 'Continue?', kind: 'confirm' });
assert.equal((await next()).hook_event_name, 'Elicitation');
emit('ui_prompt_end');
const preTurnResult = await next();
assert.equal(preTurnResult.source, 'busy');
assert.equal(preTurnResult.turn_id, submit.turn_id);
idle = false;
const input = { command: 'original' };
emit('tool_execution_start', { toolName: 'bash', toolCallId: 'tool', args: input });
input.command = 'changed';
emit('ui_prompt_start', { title: 'Approve?', kind: 'confirm' });
await flush();
assert.equal(children.length, consumed + 1, 'reports remain ordered without blocking callbacks');
assert.equal((await next()).tool_input.command, 'original');
assert.equal((await next()).hook_event_name, 'Elicitation');
// Parallel progress must not overwrite the open dialog's Waiting state.
const beforeProgress = spawns;
for (const [name, event] of [
  ['tool_execution_start', { toolName: 'bash', toolCallId: 'parallel', args: {} }],
  ['tool_execution_end', { toolName: 'bash', toolCallId: 'parallel', isError: false }],
  ['tool_execution_end', { toolName: 'bash', toolCallId: 'parallel', isError: true }],
  ['session_before_compact', {}],
  ['session_compact', { reason: 'auto' }],
  ['session_compact_failed', { reason: 'auto' }],
]) {
  emit(name, event);
  await flush();
  assert.equal(spawns, beforeProgress, `${name} must leave the dialog waiting`);
}
emit('ui_prompt_end');
const dialogResult = await next();
assert.equal(dialogResult.hook_event_name, 'ElicitationResult');
assert.equal(dialogResult.source, 'busy');
assert.equal(dialogResult.turn_id, submit.turn_id);
emit('tool_execution_end', { toolName: 'bash', toolCallId: 'tool', isError: false });
assert.equal((await next()).hook_event_name, 'PostToolUse', 'progress resumes after dismissal');
emit('message_end', { message: { role: 'assistant', content: [{ type: 'text', text: 'final' }], stopReason: 'stop' } });
emit('agent_settled');
const stop = await next();
assert.equal(stop.hook_event_name, 'Stop');
assert.equal(stop.turn_id, submit.turn_id);
assert.equal(stop.last_assistant_message, 'final');
idle = true;
emit('ui_prompt_start', { kind: 'input' });
await next();
emit('ui_prompt_end');
assert.equal((await next()).source, 'idle');
for (const outcome of ['session_compact', 'session_compact_failed']) {
  idle = false; // Pi clears manual compaction state after these events.
  emit('session_before_compact');
  assert.equal((await next()).hook_event_name, 'PreCompact');
  emit(outcome, { reason: 'manual' });
  assert.equal((await next()).source, 'idle');
}
for (const [reason, expected] of [['error', 'StopFailure'], ['aborted', 'Interrupt']]) {
  emit('before_agent_start', { prompt: reason });
  assert.notEqual((await next()).turn_id, submit.turn_id);
  emit('message_end', { message: { role: 'assistant', content: [{ type: 'text', text: 'old' }], stopReason: 'stop' } });
  emit('message_end', { message: { role: 'assistant', content: [], stopReason: reason, errorMessage: 'failure' } });
  emit('agent_settled');
  const result = await next();
  assert.equal(result.hook_event_name, expected);
  assert.equal(result.last_assistant_message, '', 'final empty failure replaces earlier text');
  idle = true;
  emit('ui_prompt_start', { kind: 'input' });
  assert.equal((await next()).turn_id, undefined, 'idle dialogs do not belong to an ended turn');
  emit('ui_prompt_end');
  assert.equal((await next()).source, 'idle');
}
// Settling a turn cannot dismiss a dialog or lose the completed turn's payload.
for (const [reason, expected] of [['stop', 'Stop'], ['error', 'StopFailure'], ['aborted', 'Interrupt']]) {
  emit('before_agent_start', { prompt: reason });
  const turn = await next();
  emit('ui_prompt_start', { title: 'Still waiting', kind: 'confirm' });
  assert.equal((await next()).hook_event_name, 'Elicitation');
  const errorMessage = reason === 'stop' ? undefined : `${reason} details`;
  emit('message_end', { message: { role: 'assistant', content: [{ type: 'text', text: `${reason} final` }],
    stopReason: reason, errorMessage } });
  const beforeSettled = spawns;
  emit('agent_settled');
  await flush();
  assert.equal(spawns, beforeSettled, `${expected} must leave the dialog waiting`);
  // Capture metadata at settlement, not at dismissal.
  session = 'changed-after-settlement';
  ctx.cwd = '/changed';
  emit('ui_prompt_end');
  const result = await next();
  assert.equal(result.hook_event_name, 'ElicitationResult');
  assert.equal(result.source, 'idle');
  assert.equal(result.turn_id, undefined, 'settlement still cleans up the active turn');
  assert.deepEqual(await next(), {
    hook_event_name: expected, session_id: turn.session_id, cwd: turn.cwd,
    turn_id: turn.turn_id, last_assistant_message: `${reason} final`,
    ...(errorMessage === undefined ? {} : { message: errorMessage }),
  });
  emit('ui_prompt_end');
  assert.equal((await next()).hook_event_name, 'ElicitationResult');
  assert.equal(children.length, consumed, 'repeated dismissal cannot replay completion');
  session = 'one';
  ctx.cwd = '/work';
}
for (const reset of ['session_start', 'before_agent_start', 'session_shutdown']) {
  emit('before_agent_start', { prompt: 'stale turn' });
  await next();
  emit('ui_prompt_start', { kind: 'confirm' });
  await next();
  emit('message_end', { message: { role: 'assistant', content: [{ type: 'text', text: 'stale' }], stopReason: 'error' } });
  emit('agent_settled');
  const resetting = emit(reset, { reason: 'new', prompt: 'replacement turn' });
  assert.equal((await next()).hook_event_name, {
    session_start: 'SessionStart', before_agent_start: 'UserPromptSubmit', session_shutdown: 'SessionEnd',
  }[reset]);
  await resetting;
  emit('ui_prompt_end');
  assert.equal((await next()).hook_event_name, 'ElicitationResult');
  assert.equal(children.length, consumed, `${reset} discards deferred completion`);
  if (reset === 'before_agent_start') {
    emit('agent_settled');
    const result = await next();
    assert.equal(result.hook_event_name, 'Stop');
    assert.equal(result.last_assistant_message, undefined);
  }
  if (reset === 'session_shutdown') handlers = load();
}
// Release runtime ownership and preserve the queue across reload and session replacement.
emit('ui_prompt_end');
let shutdown = emit('session_shutdown', { reason: 'reload' });
await next();
await shutdown;
handlers = load();
assert.ok(handlers.size);
emit('session_start', { reason: 'reload' });
assert.equal((await next()).session_id, 'one');
shutdown = emit('session_shutdown', { reason: 'new' });
assert.equal((await next()).hook_event_name, 'SessionEnd');
await shutdown;
session = 'two';
handlers = load();
emit('session_start', { reason: 'new' });
assert.equal((await next()).source, 'clear');
for (const mode of ['spawn', 'stdin', 'error', 'stdin-event']) {
  failure = mode;
  emit('ui_prompt_end');
  await flush();
  if (mode !== 'spawn') {
    const child = children[consumed++];
    if (mode === 'error') child.emit('error', Error('failed'));
    if (mode === 'stdin-event') child.stdin.emit('error', Error('broken pipe'));
    await flush();
    assert.ok(child.killed);
  }
  emit('ui_prompt_end');
  assert.equal((await next()).hook_event_name, 'ElicitationResult');
}
// Exercise the real five-second transport bound and whole-queue shutdown bound together.
emit('ui_prompt_end');
emit('ui_prompt_end');
await flush();
const stalled = children[consumed++];
const started = Date.now();
await emit('session_shutdown', { reason: 'reload' });
assert.ok(Date.now() - started < 6000, 'shutdown is bounded across the entire queue');
assert.ok(stalled.killed, 'hung reporter is killed');
// A queued report may have started at the same deadline; every started child must be bounded.
await flush();
for (const child of children.slice(consumed)) assert.ok(child.killed);
consumed = children.length;
handlers = load();
emit('session_start', { reason: 'reload' });
assert.equal((await next()).session_id, 'two', 'reporting recovers after timeout and reload');
await emit('session_shutdown', { reason: 'reload' });
console.log('Pi reporter lifecycle passed');
