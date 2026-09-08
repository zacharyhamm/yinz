// Evaluated before guest modules. The returned bridge is held only by Rust.
(() => {
  "use strict";
  const core = Deno.core;
  const dispatch = core.ops.op_zellij_command;
  const validate = core.ops.op_zellij_validate;
  const log = core.ops.op_zellij_log;
  const contexts = new WeakMap();
  const stack = [];
  // Promise context is host bookkeeping: guest prototype edits must not alter it.
  const getContext = Function.call.bind(WeakMap.prototype.get);
  const setContext = Function.call.bind(WeakMap.prototype.set);
  const pushContext = Function.call.bind(Array.prototype.push);
  const popContext = Function.call.bind(Array.prototype.pop);
  let client = null;
  let renderContext = null;
  const renderContexts = new WeakMap();
  const renderStack = [];
  core.setPromiseHooks(
    (promise, parent) => {
      setContext(contexts, promise, client ?? getContext(contexts, parent) ?? null);
      setContext(renderContexts, promise, renderContext ?? getContext(renderContexts, parent) ?? null);
    },
    (promise) => {
      pushContext(stack, client); client = getContext(contexts, promise) ?? null;
      pushContext(renderStack, renderContext); renderContext = getContext(renderContexts, promise) ?? null;
    },
    () => { client = popContext(stack) ?? null; renderContext = popContext(renderStack) ?? null; },
  );
  // No process, filesystem, networking, or raw ops are exposed to the guest.
  delete globalThis.Deno;
  delete globalThis.__bootstrap;
  // Core's console writes directly to the host's stdout. Use zellij.log instead.
  delete globalThis.console;

  let plugin = {};
  let version = 0;
  let dirty = false;
  let failure = null;
  const frames = new Map();
  let pendingRender = false;
  const handlers = new Map();
  const pipeWaiters = new Set();
  const requiredEvents = new Map();
  let nextAsync = 0n;
  let defaults = {};
  const permissionQueue = [];
  const grantedPermissions = new Set();
  const pending = new Set();
  const timers = new Set();
  let timerDeadline = null;
  function invalidate() { version++; dirty = true; }
  function fail(error) { failure ??= error instanceof Error ? error : new Error(String(error)); }
  function track(value, settled, rejected = fail) {
    if (value !== null && (typeof value === "object" || typeof value === "function") && typeof value.then === "function") {
      const promise = Promise.resolve(value);
      pending.add(promise);
      promise.then(
        (result) => { pending.delete(promise); settled(result); },
        (error) => { pending.delete(promise); rejected(error); },
      );
    } else settled(value);
  }
  function command(name, ...args) {
    if (client === null) throw new Error("host commands are unavailable during module initialization");
    let value = typeof name === "string"
      ? (args.length ? { [name]: args[0] } : name)
      : name;
    validate(value);
    if (value !== null && typeof value === "object" && Object.keys(value).length === 1 && Object.hasOwn(value, "RequestPluginPermissions")) {
      enqueuePermissions(value.RequestPluginPermissions);
      return null;
    }
    if (value !== null && typeof value === "object" && Object.keys(value).length === 1 && Object.hasOwn(value, "Unsubscribe") && Array.isArray(value.Unsubscribe)) {
      value = { Unsubscribe: value.Unsubscribe.filter((type) => !requiredEvents.has(type)) };
    }
    const response = dispatch(value, client);
    return Object.hasOwn(response, "bytes") ? new Uint8Array(response.bytes) : response.json;
  }
  function on(type, handler) {
    if (typeof handler !== "function") throw new TypeError("event handler must be a function");
    const listeners = handlers.get(type) ?? new Set();
    handlers.set(type, listeners);
    listeners.add(handler);
    return () => listeners.delete(handler);
  }
  function retainEvent(type, origin = client) {
    if (origin === null) throw new Error("host commands are unavailable during module initialization");
    dispatch({ Subscribe: [type] }, origin);
    requiredEvents.set(type, (requiredEvents.get(type) ?? 0) + 1);
    return () => {
      const count = requiredEvents.get(type) - 1;
      if (count) requiredEvents.set(type, count);
      else requiredEvents.delete(type);
      // Subscriptions stay sticky until the author explicitly unsubscribes.
    };
  }
  function cancellable(start) {
    const operation = { active: true, cleanup: () => {} };
    const promise = new Promise((resolve, reject) => {
      const settle = (finish, value) => {
        if (!operation.active) return;
        operation.active = false;
        operation.cleanup();
        finish(value);
      };
      operation.resolve = (value) => settle(resolve, value);
      operation.reject = (error) => settle(reject, error);
    });
    promise.cancel = () => {
      const error = new Error("operation cancelled");
      error.name = "AbortError";
      operation.reject(error);
    };
    try { start(operation); } catch (error) { operation.reject(error); }
    return promise;
  }
  function observeEvent(operation, type, predicate, project = (payload) => payload) {
    if (typeof type !== "string") throw new TypeError("event type must be a string");
    if (typeof predicate !== "function") throw new TypeError("event predicate must be a function");
    let release = () => {};
    const off = on(type, (payload, event) => {
      if (!operation.active) return;
      try {
        if (predicate(payload, event)) operation.resolve(project(payload));
      } catch (error) { operation.reject(error); }
    });
    operation.cleanup = () => { off(); release(); };
    release = retainEvent(type);
  }
  function waitForEvent(type, predicate = () => true) {
    return cancellable((operation) => observeEvent(operation, type, predicate));
  }
  function waitForPipe(name, predicate = () => true) {
    return cancellable((operation) => {
      if (client === null) throw new Error("pipe waits are unavailable during module initialization");
      if (typeof name !== "string") throw new TypeError("pipe name must be a string");
      if (typeof predicate !== "function") throw new TypeError("pipe predicate must be a function");
      const waiter = (message) => {
        if (!operation.active || message.name !== name) return;
        try { if (predicate(message)) operation.resolve(message); }
        catch (error) { operation.reject(error); }
      };
      pipeWaiters.add(waiter);
      operation.cleanup = () => pipeWaiters.delete(waiter);
    });
  }
  function record(value, name) {
    if (value === null || typeof value !== "object" || ![Object.prototype, null].includes(Object.getPrototypeOf(value))) {
      throw new TypeError(`${name} must be an object`);
    }
    return value;
  }
  function stringMap(value = {}, name) {
    record(value, name);
    if (Object.values(value).some((entry) => typeof entry !== "string")) {
      throw new TypeError(`${name} values must be strings`);
    }
    return { ...value };
  }
  function encodeUtf8(text) {
    if (typeof text !== "string") throw new TypeError("encodeUtf8 requires a string");
    return core.ops.op_zellij_encode(text);
  }
  function decodeUtf8(bytes) {
    if (!(bytes instanceof Uint8Array)) throw new TypeError("decodeUtf8 requires a Uint8Array");
    return core.decode(bytes);
  }
  function requestResult(operation, name, args, contextIndex, project) {
    const context = args[contextIndex];
    if (Object.hasOwn(context, "__zellij_async")) throw new TypeError("context key __zellij_async is reserved");
    const token = String(++nextAsync);
    context.__zellij_async = token;
    observeEvent(operation, `${name}Result`, (payload) => payload[3].__zellij_async === token, (payload) => {
      const context = { ...payload[3] };
      delete context.__zellij_async;
      return { ...project(payload), context };
    });
    command(name, args);
  }
  function runCommand(options) {
    return cancellable((operation) => {
      const { command: argv, envVariables, cwd = "/host", context, maxBytesReturned } = record(options, "runCommand options");
      if (!Array.isArray(argv) || !argv.length || argv.some((arg) => typeof arg !== "string") || argv[0] === "") {
        throw new TypeError("command must be a nonempty array of strings with an executable");
      }
      if (typeof cwd !== "string") throw new TypeError("cwd must be a string");
      if (maxBytesReturned !== undefined && (!Number.isSafeInteger(maxBytesReturned) || maxBytesReturned < 0)) {
        throw new TypeError("maxBytesReturned must be a nonnegative safe integer");
      }
      requestResult(operation, "RunCommand", [
        [...argv], stringMap(envVariables, "envVariables"), cwd, stringMap(context, "context"), maxBytesReturned ?? null,
      ], 3, ([exitCode, stdout, stderr]) => ({ exitCode, stdout: new Uint8Array(stdout), stderr: new Uint8Array(stderr) }));
    });
  }
  function webRequest(options) {
    return cancellable((operation) => {
      const { url, verb = "Get", headers, body = "", context, proxy, streamId } = record(options, "webRequest options");
      if (typeof url !== "string" || !url) throw new TypeError("url must be a nonempty string");
      if (!["Get", "Post", "Put", "Delete"].includes(verb)) throw new TypeError("verb must be Get, Post, Put, or Delete");
      if (typeof body !== "string" && !(body instanceof Uint8Array)) throw new TypeError("body must be a string or Uint8Array");
      if (proxy !== undefined && typeof proxy !== "string") throw new TypeError("proxy must be a string");
      if (streamId !== undefined && typeof streamId !== "string") throw new TypeError("streamId must be a string");
      requestResult(operation, "WebRequest", [
        url, verb, stringMap(headers, "headers"), Array.from(typeof body === "string" ? encodeUtf8(body) : body),
        stringMap(context, "context"), streamId ?? null, proxy ?? null,
      ], 4, ([status, headers, body]) => ({ status, headers: { ...headers }, body: new Uint8Array(body) }));
    });
  }
  function enqueuePermissions(permissions, resolve, reject) {
    if (client === null) throw new Error("host commands are unavailable during module initialization");
    if (!Array.isArray(permissions)) throw new TypeError("permissions must be an array");
    const request = { permissions: [...permissions], client, resolve, reject };
    permissionQueue.push(request);
    if (permissionQueue.length === 1) dispatchPermissions(request);
  }
  function dispatchPermissions(immediate) {
    // Permission results have no request ID. Only the queue head may listen for one.
    while (permissionQueue.length) {
      const request = permissionQueue[0];
      const permissions = [...new Set([...grantedPermissions, ...request.permissions])];
      let release = () => {};
      const off = on("PermissionRequestResult", (status) => {
        off();
        release();
        if (status === "Granted") {
          for (const permission of permissions) grantedPermissions.add(permission);
          request.resolve?.();
        } else request.reject?.(new Error("plugin permissions denied"));
        permissionQueue.shift();
        dispatchPermissions();
      });
      try {
        release = retainEvent("PermissionRequestResult", request.client);
        dispatch({ RequestPluginPermissions: permissions }, request.client);
        return;
      } catch (error) {
        off();
        release();
        permissionQueue.shift();
        if (request.reject) request.reject(error);
        else if (request === immediate) throw error;
        else fail(error);
      }
    }
  }
  function armTimer() {
    let next;
    for (const timer of timers) {
      if (timer.deadline !== Infinity && (!next || timer.deadline < next.deadline)) next = timer;
    }
    if (next && (timerDeadline === null || next.deadline < timerDeadline)) {
      dispatch({ SetTimeout: Math.max(0.001, (next.deadline - Date.now()) / 1000) }, next.client);
      timerDeadline = next.deadline;
    }
  }
  function validateTimerDuration(milliseconds) {
    if (!Number.isFinite(milliseconds) || milliseconds < 0 || milliseconds > 2147483647) {
      throw new TypeError("timer duration must be between 0 and 2147483647 milliseconds");
    }
  }
  function addTimer(milliseconds, fire, rejected = fail) {
    validateTimerDuration(milliseconds);
    const release = retainEvent("Timer");
    const timer = { deadline: Date.now() + milliseconds, client, fire, cancel: () => {
      if (timers.delete(timer)) release();
    }, reject: (error) => { timer.cancel(); rejected(error); } };
    timers.add(timer);
    try { armTimer(); } catch (error) { timer.cancel(); throw error; }
    return timer;
  }
  function sleep(milliseconds) {
    return cancellable((operation) => {
      operation.cleanup = addTimer(milliseconds, () => operation.resolve(), operation.reject).cancel;
    });
  }
  function callbackTimer(milliseconds, callback, repeat) {
    if (typeof callback !== "function") throw new TypeError("timer callback must be a function");
    const timer = addTimer(milliseconds, () => {
      const settled = (result) => {
        if (result === true) invalidate();
        if (repeat && timers.has(timer)) {
          timer.deadline = Date.now() + milliseconds;
          try { armTimer(); } catch (error) { timer.reject(error); }
        } else timer.cancel();
      };
      try { track(callback(), settled, timer.reject); } catch (error) { timer.reject(error); }
    });
    return { cancel: timer.cancel };
  }
  let agentPanes = new Map();
  let watchingAgents = false;
  function terminalId(pane) {
    if (typeof pane !== "number") validate(pane);
    const id = typeof pane === "number" ? pane :
      (pane && Object.keys(pane).length === 1 ? pane.Terminal : undefined);
    if (!Number.isSafeInteger(id) || id < 0 || id > 4294967295) throw new TypeError("agent pane must be a terminal ID fitting u32");
    return id;
  }
  function stateFilter(states) {
    const list = typeof states === "string" ? [states] : states;
    if (!Array.isArray(list) || !list.length || list.some(s => typeof s !== "string" || !["Busy", "Waiting", "Idle", "Exited", "busy", "waiting", "idle", "exited"].includes(s))) throw new TypeError("invalid agent states");
    return new Set(list.map(s => s.toLowerCase()));
  }
  function watchAgents() {
    if (!watchingAgents) { retainEvent("PaneUpdate"); watchingAgents = true; }
  }
  function observeState(operation, pane, states, {after} = {}) {
    const id = terminalId(pane), wanted = stateFilter(states);
    const preceding = after === undefined ? null : stateFilter(after);
    let seen = preceding === null;
    observeEvent(operation, "PaneUpdate", () => {
      const state = agentPanes.get(id)?.agent?.state.toLowerCase();
      if (seen && wanted.has(state)) return true;
      if (preceding?.has(state)) seen = true;
      return false;
    }, () => ({...agentPanes.get(id), agent:{...agentPanes.get(id).agent}}));
  }
  function observeTurn(operation, pane, {fallback} = {}) {
    const id = terminalId(pane);
    if (fallback !== undefined && fallback !== "idle") throw new TypeError('fallback must be "idle"');
    observeEvent(operation, "AgentEvent", e => e.pane_id === id && ["Stop", "StopFailure", "Interrupt"].includes(e.hook),
      e => ({message:e.last_message, hook:e.hook, state:e.state, sessionId:e.session_id}));
    if (fallback === "idle") {
      const cleanup = operation.cleanup;
      const idle = {
        get active() { return operation.active; }, cleanup: () => {}, reject: operation.reject,
        resolve: p => operation.resolve({message:null, hook:null, state:"Idle", sessionId:p.agent.session_id ?? null}),
      };
      operation.cleanup = () => { cleanup(); idle.cleanup(); };
      observeState(idle, id, "Idle", {after:["Busy", "Waiting"]});
    }
  }
  function validateAgentSend(pane, text, {enter = true} = {}) {
    const target = {Terminal:terminalId(pane)};
    if (typeof text !== "string" || typeof enter !== "boolean") throw new TypeError("agent send requires text and a boolean enter option");
    return {target, enter};
  }
  const agent = Object.freeze({
    spawnDetails(options) {
      validate(options); record(options, "agent spawn options");
      const {sourceDirectory, cwd, name, context = {}, automaticFirecrackerVm = false, startSuppressed = false, ...spec} = options;
      return command("SpawnAgent", {spec, source_directory:sourceDirectory ?? null, cwd:cwd ?? null, name:name ?? null,
        context, automatic_firecracker_vm:automaticFirecrackerVm, start_suppressed:startSuppressed});
    },
    spawn(options) { return agent.spawnDetails(options).pane_id; },
    info(pane) { const id = terminalId(pane); watchAgents(); return agentPanes.get(id)?.agent ? {...agentPanes.get(id).agent} : null; },
    all() { watchAgents(); return Object.fromEntries([...agentPanes].filter(([,p]) => p.agent).map(([id,p]) => [id,{...p.agent}])); },
    on(handler) {
      if (typeof handler !== "function") throw new TypeError("agent handler must be a function");
      const release = retainEvent("AgentEvent"), off = on("AgentEvent", handler);
      let active = true;
      return () => { if (active) { active = false; off(); release(); } };
    },
    send(pane, text, options) {
      const {target, enter} = validateAgentSend(pane, text, options);
      command("WriteToPaneId", [encodeUtf8("\x1b[200~" + text + "\x1b[201~"), target]);
      if (enter) command("WriteToPaneId", [encodeUtf8("\r"), target]);
    },
    waitForState: (pane, states, options) => cancellable(op => observeState(op, pane, states, options)),
    waitForEvent(pane, hooks) {
      return cancellable(op => {
        const id = terminalId(pane), list = typeof hooks === "string" ? [hooks] : hooks;
        if (list !== undefined && (!Array.isArray(list) || list.some(h => typeof h !== "string"))) throw new TypeError("hooks must be a string or array of strings");
        const wanted = list === undefined ? null : new Set(list);
        observeEvent(op, "AgentEvent", e => e.pane_id === id && (wanted === null || wanted.has(e.hook)));
      });
    },
    waitForTurn: (pane, options) => cancellable(op => observeTurn(op, pane, options)),
    prompt: (pane, text, options) => cancellable(op => { validateAgentSend(pane, text, options); observeTurn(op, pane, options); agent.send(pane, text, options); }),
  });

  const fs = Object.freeze({
    read: (path, {maxBytes = null} = {}) => command('FsRead', {path, max_bytes: maxBytes}),
    list: (path) => command('FsList', {path}),
    metadata: (path) => command('FsMetadata', {path}),
    write(path, contents, {append = false, createDirs = false} = {}) {
      if (typeof contents !== 'string' && !(contents instanceof Uint8Array)) throw new TypeError('file contents must be a string or Uint8Array');
      command('FsWrite', {path, contents: typeof contents === 'string' ? encodeUtf8(contents) : contents, append, create_dirs: createDirs});
    },
    remove(path, {recursive = false} = {}) { command('FsRemove', {path, recursive}); },
    mkdir(path, {all = false} = {}) { command('FsCreateDir', {path, all}); },
    rename(from, to) { command('FsRename', {from, to}); },
  });
  const projectConfig = Object.freeze({
    read: (directory) => command('ProjectConfigRead', {directory}),
    write(directory, source, expectedSource) {
      if (expectedSource !== null && typeof expectedSource !== 'string') throw new TypeError('expectedSource is required and must be a string or null');
      return command('ProjectConfigWrite', {directory, source, expected: expectedSource});
    },
  });

  function int(value) {
    if ((typeof value !== 'bigint' && !Number.isSafeInteger(value)) || value < -9223372036854775808n || value > 9223372036854775807n) {
      throw new TypeError('data integer must be exact and fit i64');
    }
    return {Int: value};
  }
  function real(value) {
    if (typeof value !== 'number' || !Number.isFinite(value)) throw new TypeError('data real must be a finite number');
    return {Real: value};
  }
  function toValue(value) {
    if (value === null) return 'Null';
    if (typeof value === 'string') return {Text: value};
    if (typeof value === 'boolean') return {Bool: value};
    if (typeof value === 'bigint' || Number.isSafeInteger(value)) return int(value);
    if (typeof value === 'number') return real(value);
    if (value instanceof Uint8Array) return {Blob: value};
    record(value, 'data value');
    if (Object.keys(value).length === 1) {
      if (Object.hasOwn(value, 'Int')) return int(value.Int);
      if (Object.hasOwn(value, 'Real')) return real(value.Real);
      if (typeof value.Text === 'string') return {Text: value.Text};
      if (typeof value.Bool === 'boolean') return {Bool: value.Bool};
      if (value.Blob instanceof Uint8Array || Array.isArray(value.Blob)) { validate(value); return {Blob: value.Blob}; }
    }
    throw new TypeError('unsupported data value; use data.json() for JSON content');
  }
  function fromValue(value) {
    if (value === 'Null') return null;
    if (Object.hasOwn(value, 'Blob')) return new Uint8Array(value.Blob);
    return value.Int ?? value.Real ?? value.Text ?? value.Bool;
  }
  function fromRecord(values) { return Object.entries(record(values, 'data record')).map(([name, value]) => [name, toValue(value)]); }
  function toRecords(raw) {
    return raw.rows.map((row) => Object.fromEntries(raw.columns.map((name, index) => [name, fromValue(row[index])])));
  }
  function snapshot(raw) { return {records: toRecords(raw), raw}; }
  function query(options) {
    const {entity, namespace = null, filter = null, search = null, sort = [], select = [], limit = 100, offset = 0, afterId = null} = record(options, 'data query');
    return {entity, namespace, filter, search, sort, select, limit, offset, after_id: afterId};
  }
  const data = Object.freeze({
    define(name, attributes, {namespace = null, version = 1, allowDestructive = false} = {}) {
      const attrs = Object.entries(record(attributes, 'entity attributes')).map(([name, options]) => {
        const {kind, indexed = false, fullText = false, unique = false, required = false, default: fallback} = typeof options === 'string' ? {kind: options} : record(options, 'attribute');
        return {name, kind, indexed, full_text: fullText, unique, required, default: fallback === undefined ? null : toValue(fallback)};
      });
      return command('DataDefineEntity', {namespace, def: {name, version, attrs}, allow_destructive: allowDestructive});
    },
    catalog: (namespace = null) => command('DataCatalog', {namespace}),
    fetch: (options) => snapshot(command('DataFetch', {request: query(options)})),
    count: (options) => command('DataCount', {request: query(options)}),
    apply: (mutations, {namespace = null} = {}) => command('DataApply', {namespace, mutations}),
    put: (entity, values, {uid = null, expectRev = null} = {}) => ({Put: {entity, uid, values: fromRecord(values), expect_rev: expectRev}}),
    update: (entity, filter, values, {expectRev = null} = {}) => ({Update: {entity, filter, values: fromRecord(values), expect_rev: expectRev}}),
    delete: (entity, filter) => ({Delete: {entity, filter}}),
    eq: (column, value) => ({Eq: [column, toValue(value)]}),
    ne: (column, value) => ({Ne: [column, toValue(value)]}),
    gt: (column, value) => ({Gt: [column, toValue(value)]}),
    gte: (column, value) => ({Gte: [column, toValue(value)]}),
    lt: (column, value) => ({Lt: [column, toValue(value)]}),
    lte: (column, value) => ({Lte: [column, toValue(value)]}),
    in: (column, values) => ({In: [column, values.map(toValue)]}),
    isNull: (column) => ({IsNull: column}), notNull: (column) => ({NotNull: column}),
    prefix: (column, value) => ({Prefix: [column, value]}), contains: (column, value) => ({Contains: [column, value]}),
    and: (...filters) => ({And: filters}), or: (...filters) => ({Or: filters}), not: (filter) => ({Not: filter}),
    toValue, fromValue, fromRecord, toRecords, int, real,
    json(value) {
      validate(value);
      const text = JSON.stringify(value);
      if (text === undefined) throw new TypeError('data.json requires JSON content');
      return {Text: text};
    },
    isConflict: (error) => (error instanceof Error ? error.message : String(error)).includes('data conflict: '),
    watch(options, onChange, {mode = 'Rows'} = {}) {
      if (typeof onChange !== 'function') throw new TypeError('watch callback must be a function');
      const origin = client;
      let watchId = null, release = () => {}, disposed = false;
      const off = on('PluginDataChanged', (event) => {
        if (!disposed && event.watch_id === watchId) return onChange(snapshot(event.rows), event);
      });
      try {
        release = retainEvent('PluginDataChanged', origin);
        const handle = command('DataWatch', {request: query(options), mode});
        watchId = handle.watch_id;
        return {watchId, initial: snapshot(handle.rows), dispose() {
          if (disposed) return;
          dispatch({DataUnwatch: {watch_id: watchId}}, origin);
          disposed = true;
          off(); release();
        }};
      } catch (error) { off(); release(); throw error; }
    },
  });

  let hocusIdentity = null;
  let hocusDefinitions = null;
  let hocusHandlers = new Map();
  let rpcPrefix = null;
  let rpcCount = 0n;
  const rpcHandlers = new Map();
  const rpcCalls = new Map();
  function jsonMessage(value) {
    // The host rejects getters, proxies, cycles, sparse arrays and excessive depth.
    validate(value);
    function copy(value) {
      if (typeof value === "bigint" || value instanceof Uint8Array) throw new TypeError("RPC requires JSON values");
      if (value === null || typeof value !== "object") return value;
      if (Array.isArray(value)) return value.map(copy);
      return Object.fromEntries(Object.getOwnPropertyNames(value).map((key) => {
        if (!Object.getOwnPropertyDescriptor(value, key).enumerable) throw new TypeError("JSON properties must be enumerable");
        return [key, copy(value[key])];
      }));
    }
    const encoded = JSON.stringify(copy(value));
    if (encodeUtf8(encoded).length > 64000) throw new RangeError("RPC/Hocus payload exceeds 64000 bytes");
    return encoded;
  }
  function sendPluginMessage(name, payload, destination = null, pluginUrl = null, pluginConfig = {}) {
    const encoded = jsonMessage(payload);
    command("MessageToPlugin", {
      plugin_url: pluginUrl, destination_plugin_id: destination, plugin_config: pluginConfig,
      message_name: name, message_payload: encoded, message_args: {},
      new_plugin_args: null, floating_pane_coordinates: null,
    });
  }
  const rpc = Object.freeze({
    handle(name, handler) {
      if (typeof name !== "string" || !name || name.startsWith("hocus_tool:")) throw new TypeError("RPC name must be nonempty and not use reserved hocus_tool: prefix");
      if (typeof handler !== "function") throw new TypeError("RPC handler must be a function");
      rpcHandlers.set(name, handler);
    },
    call(options) {
      return cancellable((operation) => {
        const {pluginId, pluginUrl, pluginConfig, name, payload = null, timeoutMs} = record(options, "RPC options");
        for (const key of Object.keys(options)) if (!["pluginId", "pluginUrl", "pluginConfig", "name", "payload", "timeoutMs"].includes(key)) throw new TypeError("unknown RPC option: " + key);
        if ((pluginId === undefined) === (pluginUrl === undefined)) throw new TypeError("RPC requires exactly one destination");
        if (pluginId !== undefined && (!Number.isSafeInteger(pluginId) || pluginId < 0 || pluginId > 4294967295)) throw new TypeError("pluginId must fit u32");
        if (pluginUrl !== undefined && (typeof pluginUrl !== "string" || !pluginUrl)) throw new TypeError("pluginUrl must be nonempty");
        if (pluginId !== undefined && pluginConfig !== undefined) throw new TypeError("pluginConfig requires pluginUrl");
        if (typeof name !== "string" || !name) throw new TypeError("RPC name must be nonempty");
        if (rpcPrefix === null) throw new Error("RPC isolate identity is unavailable");
        const config = stringMap(pluginConfig, "pluginConfig");
        const callId = rpcPrefix + ":rpc" + (++rpcCount);
        let timer;
        operation.cleanup = () => { rpcCalls.delete(callId); timer?.cancel(); };
        rpcCalls.set(callId, {operation, pluginId});
        if (timeoutMs !== undefined) timer = addTimer(timeoutMs, () => {
          const error = new Error("RPC timed out: " + name);
          error.name = "TimeoutError";
          operation.reject(error);
        }, operation.reject);
        sendPluginMessage("zellij_rpc_call", {version: 1, call_id: callId, name, payload}, pluginId ?? null, pluginUrl ?? null, config);
      });
    },
  });
  function registerHocus(definitions = hocusDefinitions) {
    if (hocusIdentity && definitions !== null) {
      sendPluginMessage("hocus_extension_register", {version: 1, ...hocusIdentity, tools: definitions});
    }
  }
  const completions = new Map();
  let completionCount = 0n;
  function completionCalls(calls) {
    if (!Array.isArray(calls)) throw new TypeError("tool_calls must be an array");
    for (const call of calls) {
      record(call, "tool call");
      if (typeof call.id !== "string" || typeof call.name !== "string" || (call.arguments !== undefined && typeof call.arguments !== "string")) throw new TypeError("invalid completion tool call");
    }
  }
  function complete(options) {
    return cancellable(operation => {
      // Validate the entire guest graph before destructuring or reading any field.
      validate(options); record(options, "completion options");
      for (const key of Object.keys(options)) if (!["messages", "system", "model", "tools", "toolChoice", "temperature", "maxTokens", "timeoutMs"].includes(key)) throw new TypeError("unknown completion option: " + key);
      const {messages, system = null, model = null, tools = [], toolChoice = null, temperature = null, maxTokens = null, timeoutMs = 60000} = options;
      if (!Array.isArray(messages) || messages.length > 200 || (!messages.length && system === null)) throw new TypeError("completion requires messages (at most 200) or a system prompt");
      if (!Array.isArray(tools) || tools.length > 64) throw new TypeError("completion allows at most 64 tools");
      if ((system !== null && typeof system !== "string") || (model !== null && typeof model !== "string")) throw new TypeError("system and model must be strings");
      if (temperature !== null && !Number.isFinite(temperature)) throw new TypeError("temperature must be finite");
      if (maxTokens !== null && (!Number.isSafeInteger(maxTokens) || maxTokens < 0 || maxTokens > 4294967295)) throw new TypeError("maxTokens must fit u32");
      validateTimerDuration(timeoutMs);
      for (const message of messages) {
        record(message, "completion message");
        if (!["system", "user", "assistant", "tool"].includes(message.role) || (message.content !== undefined && typeof message.content !== "string")) throw new TypeError("invalid completion message");
        if ((message.tool_call_id != null && typeof message.tool_call_id !== "string") || (message.role === "tool" && typeof message.tool_call_id !== "string")) throw new TypeError("tool messages require tool_call_id");
        completionCalls(message.tool_calls === undefined ? [] : message.tool_calls);
      }
      if (rpcPrefix === null) throw new Error("completion isolate identity is unavailable");
      const request_id = rpcPrefix + ":hc" + (++completionCount);
      const payload = {request_id, messages, system, model, tools, tool_choice:toolChoice, temperature, max_tokens:maxTokens};
      jsonMessage(payload);
      const snapshot = command("GetSessionList");
      const daemons = snapshot.live_sessions.filter(s => s.is_current_session).flatMap(s => Object.entries(s.plugins))
        .filter(([,p]) => p.location === "zellij:hocus-agentd");
      if (daemons.length !== 1) throw new Error(daemons.length ? "multiple hocus-agentd plugins in the current session" : "no hocus-agentd in the current session");
      const pluginId = Number(daemons[0][0]);
      if (!Number.isSafeInteger(pluginId) || pluginId < 0 || pluginId > 4294967295) throw new Error("invalid hocus-agentd plugin ID");
      let timer;
      operation.cleanup = () => { completions.delete(request_id); timer?.cancel(); };
      completions.set(request_id, {operation, pluginId});
      timer = addTimer(timeoutMs, () => {
        const error = new Error("Hocus completion timed out"); error.name = "TimeoutError"; operation.reject(error);
      }, operation.reject);
      sendPluginMessage("hocus_complete", payload, pluginId);
    });
  }
  function completionPipe(message) {
    if (message.name === "hocus_complete") return true;
    if (message.name !== "hocus_complete_result") return false;
    if (typeof message.payload !== "string") return true;
    let reply;
    try { reply = JSON.parse(message.payload); validate(reply); record(reply, "completion reply"); } catch (_) { return true; }
    const waiting = completions.get(reply.request_id);
    if (!waiting || message.source?.Plugin !== waiting.pluginId) return true;
    try {
      if (typeof reply.error !== "string") throw new TypeError("invalid completion error");
      if (reply.error) { waiting.operation.reject(new Error(reply.error)); return true; }
      if ([reply.content, reply.finish_reason, reply.model].some(v => typeof v !== "string")) throw new TypeError("invalid completion result");
      completionCalls(reply.tool_calls);
      if (reply.tool_calls.some(c => typeof c.arguments !== "string")) throw new TypeError("invalid tool arguments");
      if (reply.usage !== null) {
        record(reply.usage, "completion usage");
        if (["prompt_tokens", "completion_tokens", "total_tokens"].some(k => !Number.isSafeInteger(reply.usage[k]) || reply.usage[k] < 0)) throw new TypeError("invalid completion usage");
      }
    } catch (_) { return true; }
    waiting.operation.resolve({content:reply.content, toolCalls:reply.tool_calls, finishReason:reply.finish_reason, usage:reply.usage, model:reply.model});
    return true;
  }
  const hocus = Object.freeze({
    complete,
    launch: () => hocusIdentity && {...hocusIdentity},
    tools(definitions) {
      record(definitions, "Hocus tools");
      const entries = Object.entries(definitions);
      if (entries.length > 32) throw new RangeError("at most 32 Hocus tools are allowed");
      const replacement = new Map();
      const specs = entries.sort(([a], [b]) => a.localeCompare(b)).map(([name, definition]) => {
        const {handler, description = name, parameters = {type: "object"}, safety = "read_only", consequence = null} = record(definition, "Hocus tool");
        if (!/^[a-z][a-z0-9_]*$/.test(name) || name.length > 64) throw new TypeError("tool names must be lower snake case, at most 64 characters");
        if (typeof handler !== "function") throw new TypeError("tool handler must be a function");
        if (typeof description !== "string" || !description.trim() || encodeUtf8(description).length > 2000) throw new TypeError("tool description must contain 1–2000 bytes");
        validate(parameters);
        if (parameters?.type !== "object" || encodeUtf8(JSON.stringify(parameters)).length > 16000) throw new TypeError("tool parameters must be an object JSON Schema of at most 16000 bytes");
        if (!["read_only", "automatic", "confirm"].includes(safety)) throw new TypeError("invalid tool safety");
        if (consequence !== null && typeof consequence !== "string") throw new TypeError("consequence must be a string");
        if (safety === "confirm" && !consequence?.trim()) throw new TypeError("confirm requires a consequence");
        replacement.set("hocus_tool:" + name, handler);
        return {name, description, parameters, safety, consequence};
      });
      // The dispatcher checks the existing grant before publishing or replacing handlers.
      registerHocus(specs);
      hocusDefinitions = JSON.parse(JSON.stringify(specs));
      hocusHandlers = replacement;
    },
  });
  function hocusPipe(message) {
    if (message.name === "hocus_extension_discover") {
      if (message.source && Number.isSafeInteger(message.source.Plugin)) registerHocus();
      return true;
    }
    return false;
  }
  function rpcPipe(message) {
    if (!["zellij_rpc_call", "zellij_rpc_reply"].includes(message.name)) return false;
    const from = message.source?.Plugin;
    if (!Number.isSafeInteger(from) || from < 0 || from > 4294967295 || typeof message.payload !== "string" || encodeUtf8(message.payload).length > 64000) return true;
    let call;
    try { call = JSON.parse(message.payload); validate(call); } catch (_) { return true; }
    if (call?.version !== 1 || typeof call.call_id !== "string" || !call.call_id) return true;
    if (message.name === "zellij_rpc_reply") {
      const waiting = rpcCalls.get(call.call_id);
      if (!waiting || (waiting.pluginId !== undefined && waiting.pluginId !== from)) return true;
      const hasError = Object.hasOwn(call, "error"), hasResult = Object.hasOwn(call, "result");
      if (hasError && (hasResult || typeof call.error !== "string")) return true;
      // Lua omits the result field when its handler returns nil.
      if (hasError) waiting.operation.reject(new Error(call.error));
      else waiting.operation.resolve(call.result ?? null);
      return true;
    }
    if (typeof call.name !== "string" || !call.name) return true;
    const origin = client;
    const reply = (result, error) => {
      const previous = client;
      client = origin;
      try {
        let payload;
        try {
          payload = {version: 1, call_id: call.call_id, ...(error === undefined ? {result: result === undefined ? null : result} : {error: String(error).slice(0, 2000)})};
          jsonMessage(payload);
        } catch (error) {
          payload = {version: 1, call_id: call.call_id, error: String(error).slice(0, 2000)};
        }
        // Leave room for the opaque caller ID even when it nearly fills the envelope.
        if (Object.hasOwn(payload, "error")) {
          while (encodeUtf8(JSON.stringify(payload)).length > 64000 && payload.error.length) payload.error = payload.error.slice(0, Math.floor(payload.error.length / 2));
        }
        sendPluginMessage("zellij_rpc_reply", payload, from);
      } catch (error) { fail(error); }
      finally { client = previous; }
    };
    const handler = call.name.startsWith("hocus_tool:") ? (hocusIdentity && hocusHandlers.get(call.name)) : rpcHandlers.get(call.name);
    try {
      if (!handler) throw new Error("no rpc handler named " + call.name);
      const payload = call.payload ?? null;
      track(handler(payload, {version:1, callId:call.call_id, name:call.name, payload, source:{Plugin:from}}), (result) => reply(result), (error) => reply(null, error ?? "RPC handler failed"));
    } catch (error) { reply(null, error ?? "RPC handler failed"); }
    return true;
  }
  function uiInteger(n, name) {
    if (!Number.isSafeInteger(n) || n < 0 || n > 262144) throw new RangeError(`${name} must be an integer between 0 and 262144`);
    return n;
  }
  function coordinates(c) {
    validate(c);
    const {x = 0, y = 0, width, height, scroll = 0} = record(c, 'coordinates');
    for (const key of Object.keys(c)) if (!['x','y','width','height','scroll'].includes(key)) throw new TypeError('unknown coordinate: ' + key);
    return {x: uiInteger(x,'x'), y: uiInteger(y,'y'), width: uiInteger(width,'width'), height: uiInteger(height,'height'), scroll: uiInteger(scroll,'scroll')};
  }
  const pure = (op, args) => core.ops.op_zellij_ui_pure({op, args});
  const text = Object.freeze({
    width: (s) => pure('Width', s), sanitize: (s) => pure('Sanitize', s),
    wrap: (s, width) => pure('Wrap', [s, uiInteger(width,'width')]),
    truncate: (s, width, ellipsis = '…') => pure('Truncate', [s, uiInteger(width,'width'), ellipsis]),
    pad: (s, width) => pure('Pad', [s, uiInteger(width,'width')]),
    padLeft: (s, width) => pure('PadLeft', [s, uiInteger(width,'width')]),
    center: (s, width) => pure('Center', [s, uiInteger(width,'width')]),
  });
  const styled = new WeakMap();
  function textDescriptor(value) {
    if (styled.has(value)) return structuredStyle(styled.get(value));
    if (typeof value !== 'string') throw new TypeError('component text must be a string or styled text');
    return {text:value, selected:false, opaque:false, disabled:false, levels:[]};
  }
  function structuredStyle(d) { return {...d, levels:d.levels.map(indices => [...indices])}; }
  function styledText(value = '', ribbon = false) {
    const d = textDescriptor(value);
    const w = {
      len: () => [...d.text].length,
      selected() { d.selected = true; return w; },
      opaque() { d.opaque = true; return w; },
      disabled() { d.disabled = true; return w; },
      emphasis(level, from = 0, to = w.len()) {
        uiInteger(level,'level'); uiInteger(from,'from'); uiInteger(to,'to');
        if (level > 7 || from > to || to > w.len()) throw new RangeError('style range or level out of bounds');
        while (d.levels.length <= level) d.levels.push([]);
        for (let i = from; i < to; i++) d.levels[level].push(i);
        return w;
      },
      dim: (from,to) => w.emphasis(4,from,to), unbold: (from,to) => w.emphasis(5,from,to),
      error: (from,to) => w.emphasis(6,from,to), success: (from,to) => w.emphasis(7,from,to),
      render: (c) => pure(ribbon ? 'Ribbon' : 'Text', [d, c === undefined ? null : coordinates(c)]),
    };
    styled.set(w,d);
    return w;
  }
  function fieldDescriptor(d, single) {
    validate(d);
    record(d,'field descriptor');
    for (const k of Object.keys(d)) if (!['kind','id','label','value','options','placeholder','maxVisibleOptions','disabled','style'].includes(k)) throw new TypeError('unknown field descriptor: ' + k);
    const kinds = {textInput:'text_input', radioGroup:'radio_group', checkbox:'checkbox', select:'select', combo:'combo', button:'button'};
    if (!Object.hasOwn(kinds,d.kind)) throw new TypeError('unknown widget kind');
    return {kind:kinds[d.kind], id:d.id ?? (single ? 'value' : null), label:d.label ?? '',
      value:Object.hasOwn(d,'value') ? d.value : (['textInput','combo'].includes(d.kind) ? '' : d.kind === 'checkbox' ? false : d.options?.length ? 0 : null),
      options:d.options ?? [], placeholder:d.placeholder ?? '', max_visible_options:d.maxVisibleOptions ?? 6,
      disabled:d.disabled ?? false, style:d.style ?? 'ribbon'};
  }
  function construct(fields, single = false, dialog = null) {
    if (!Array.isArray(fields)) throw new TypeError('fields must be an array');
    const descriptors = fields.map(d => fieldDescriptor(d,single));
    const state = core.ops.op_zellij_ui_create({kind:'Form', fields:descriptors, dialog});
    const singleId = single ? descriptors[0].id : undefined;
    const call = (op, args) => core.ops.op_zellij_ui_call(state, args === undefined ? {op} : {op,args});
    const w = {
      value: (id = singleId) => call('Value',id), values: () => call('Values'),
      setValue(id, value) { call('SetValue',single ? [singleId,id] : [id,value]); return w; },
      setOptions(id, options) { call('SetOptions',single ? [singleId,id] : [id,options]); return w; },
      focus(id = singleId) { call('Focus',id); return w; }, focusedId: () => call('FocusedId'),
      moveFocus: (forward = true) => call('MoveFocus',forward),
      handleKey: (key) => call('Key',key), handleMouse: (mouse) => call('Mouse',mouse),
      insertPastedText: (text) => call('Paste',text),
      render(c) {
        const result = call('Render',coordinates(c));
        if (result.updateCursor && renderContext !== null) renderContext.cursor = result.cursor;
        return result.text;
      },
    };
    if (dialog) w.setStatus = (text = null) => { call('SetStatus',text); return w; };
    return w;
  }
  const ui = {
    text: (s) => styledText(s), ribbon: (s) => styledText(s,true),
    form: (fields) => construct(fields),
    dialog(d) {
      validate(d); record(d,'dialog');
      for (const k of Object.keys(d)) if (!['fields','title','message','help'].includes(k)) throw new TypeError('unknown dialog field: ' + k);
      return construct(d.fields ?? [],false,{title:d.title, message:d.message ?? null, help:d.help ?? null});
    },
    markdown(source, {compact = false} = {}) {
      const state = core.ops.op_zellij_ui_create({kind:'Markdown',source,compact});
      return {render: (c) => core.ops.op_zellij_ui_call(state,{op:'Render',args:coordinates(c)})};
    },
    table(rows = []) {
      if (!Array.isArray(rows)) throw new TypeError('table rows must be an array');
      rows = rows.map(row => row.map(textDescriptor));
      const w = {row(...cells) { rows.push(cells.map(textDescriptor)); return w; },
        render: (c) => pure('Table',[rows,c === undefined ? null : coordinates(c)])};
      return w;
    },
    nestedList(items = []) {
      const entry = (item) => typeof item === 'string' || styled.has(item) ? [textDescriptor(item),0] : [textDescriptor(item.text),uiInteger(item.indent ?? 0,'indent')];
      items = items.map(entry);
      const w = {item(text, indent = 0) { items.push(entry({text,indent})); return w; },
        render: (c) => pure('NestedList',[items,c === undefined ? null : coordinates(c)])};
      return w;
    },
  };
  for (const kind of ['textInput','checkbox','radioGroup','select','combo','button']) ui[kind] = (d = {}) => construct([{...d,kind}],true);
  Object.freeze(ui);

  const api = Object.freeze({
    command,
    definePlugin: (value) => value,
    subscribe: (events) => command("Subscribe", events),
    unsubscribe: (events) => command("Unsubscribe", events),
    requestPermission: (permissions) => command("RequestPluginPermissions", permissions),
    requestRender: invalidate,
    on,
    sleep,
    after: (milliseconds, callback) => callbackTimer(milliseconds, callback, false),
    every: (milliseconds, callback) => callbackTimer(milliseconds, callback, true),
    runCommand,
    webRequest,
    fs,
    projectConfig,
    ui,
    text,
    hocus,
    agent,
    rpc,
    data,
    encodeUtf8,
    decodeUtf8,
    waitForPipe,
    getPluginIds: () => command("GetPluginIds"),
    log: (...values) => {
      if (client === null) throw new Error("logging is unavailable during module initialization");
      log(values.map(String).join("\t"));
    },
    waitForEvent,
    waitForPermissions: (permissions) => new Promise((resolve, reject) => enqueuePermissions(permissions, resolve, reject)),
  });
  Object.defineProperty(globalThis, "zellij", { value: api });

  function lifecycle(name, arg) {
    if (name === "load") {
      const namespace = arg.hocus_extension_name, path = arg.hocus_extension_path;
      hocusIdentity = typeof namespace === "string" && namespace && typeof path === "string" && path ? {namespace, path} : null;
    }
    if (name === "pipe" && (completionPipe(arg) || rpcPipe(arg) || hocusPipe(arg))) return;
    const type = name === "update" ? (typeof arg === "string" ? arg : Object.keys(arg)[0]) : undefined;
    const payload = name === "update" && typeof arg !== "string" ? arg[type] : undefined;
    if (type === "PaneUpdate") {
      agentPanes = new Map(Object.values(payload.panes).flat().filter(p => !p.is_plugin).map(p => [p.id, {...p, agent:p.agent && {...p.agent}}]));
    }
    const listeners = [...(handlers.get(type) ?? [])];
    const waiters = name === "pipe" ? [...pipeWaiters] : [];
    // Starts are ordered by the executor. Promises are observed, never awaited here:
    // load may be waiting for the very permission event that update must deliver.
    if (typeof plugin[name] === "function") {
      track(plugin[name](name === "load" ? { ...defaults, ...arg } : arg), (result) => { if (result === true || name === "load") invalidate(); });
    }
    for (const waiter of waiters) waiter(arg);
    if (name === "update") {
      if (type === "Timer") {
        const now = Date.now();
        // Timer events have no ID. A stale/early one must still rearm the next
        // deadline; wall-clock rounding must never strand a sleep forever.
        timerDeadline = null;
        for (const timer of [...timers]) {
          if (!timers.has(timer) || timer.deadline > now) continue;
          timer.deadline = Infinity;
          const previous = client;
          client = timer.client;
          try { timer.fire(); } finally { client = previous; }
        }
        try { armTimer(); } catch (error) {
          for (const timer of [...timers]) {
            if (timer.deadline !== Infinity) timer.reject(error);
          }
        }
      }
      for (const handler of listeners) {
        track(handler(payload, arg), (result) => { if (result === true) invalidate(); });
      }
    }
  }
  function render(rows, columns) {
    let frame = frames.get(client);
    if (!frame) {
      frame = { rows, columns, version: -1, pending: false, text: "" };
      frames.set(client, frame);
    }
    if (frame.rows !== rows || frame.columns !== columns) {
      frame.rows = rows;
      frame.columns = columns;
      frame.version = -1;
    }
    if (!pendingRender && frame.version !== version) {
      const startedVersion = version;
      const invocation = {cursor: undefined};
      frame.pending = true;
      pendingRender = true;
      let synchronous = true;
      const finish = (text) => {
        frame.pending = false;
        pendingRender = false;
        if (typeof text !== "string" && text !== undefined && text !== null) {
          throw new TypeError("render must return a string, null, or undefined");
        }
        if (startedVersion === version && frame.rows === rows && frame.columns === columns) {
          frame.text = text ?? "";
          frame.cursor = invocation.cursor;
          frame.version = startedVersion;
        }
        // Publish completion, or start the replacement for a superseded render.
        if (!synchronous || frame.version !== version) dirty = true;
      };
      try {
        const previousRender = renderContext;
        let value;
        renderContext = invocation;
        try { value = typeof plugin.render === "function" ? plugin.render(rows, columns) : ""; }
        finally { renderContext = previousRender; }
        track(value, (text) => { try { finish(text); } catch (error) { fail(error); } });
        synchronous = false;
      } catch (error) { frame.pending = false; pendingRender = false; throw error; }
    }
    if (frame.version === version && frame.cursor !== undefined) command('ShowCursor', frame.cursor);
    return frame.text;
  }
  return (action, arg, origin) => {
    const previous = client;
    client = origin ?? null;
    try {
      if (failure) { const error = failure; failure = null; throw error; }
      if (action === "initialize") {
        if (rpcPrefix !== null) throw new Error("SDK already initialized");
        rpcPrefix = arg;
      } else if (action === "bind") {
        plugin = arg.default;
        if (plugin === null || typeof plugin !== "object") {
          throw new TypeError("plugin must default-export a lifecycle object");
        }
        for (const name of ["load", "update", "pipe", "render"]) {
          if (plugin[name] !== undefined && typeof plugin[name] !== "function") {
            throw new TypeError(`${name} must be a function`);
          }
        }
        defaults = stringMap(plugin.defaults, "defaults");
      } else if (action === "render") return render(...arg);
      else if (action === "peek") return dirty;
      else if (action === "status") { const requested = dirty; dirty = false; return requested; }
      else lifecycle(action, arg);
      return null;
    } finally { client = previous; }
  };
})()
