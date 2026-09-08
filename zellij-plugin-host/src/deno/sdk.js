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
  core.setPromiseHooks(
    (promise, parent) => setContext(contexts, promise, client ?? getContext(contexts, parent) ?? null),
    (promise) => { pushContext(stack, client); client = getContext(contexts, promise) ?? null; },
    () => { client = popContext(stack) ?? null; },
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
      const { url, verb = "Get", headers, body = "", context, proxy } = record(options, "webRequest options");
      if (typeof url !== "string" || !url) throw new TypeError("url must be a nonempty string");
      if (!["Get", "Post", "Put", "Delete"].includes(verb)) throw new TypeError("verb must be Get, Post, Put, or Delete");
      if (typeof body !== "string" && !(body instanceof Uint8Array)) throw new TypeError("body must be a string or Uint8Array");
      if (proxy !== undefined && typeof proxy !== "string") throw new TypeError("proxy must be a string");
      requestResult(operation, "WebRequest", [
        url, verb, stringMap(headers, "headers"), Array.from(typeof body === "string" ? encodeUtf8(body) : body),
        stringMap(context, "context"), null, proxy ?? null,
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
  function addTimer(milliseconds, fire, rejected = fail) {
    if (!Number.isFinite(milliseconds) || milliseconds < 0 || milliseconds > 2147483647) {
      throw new TypeError("timer duration must be between 0 and 2147483647 milliseconds");
    }
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
    const type = name === "update" ? (typeof arg === "string" ? arg : Object.keys(arg)[0]) : undefined;
    const payload = name === "update" && typeof arg !== "string" ? arg[type] : undefined;
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
          frame.version = startedVersion;
        }
        // Publish completion, or start the replacement for a superseded render.
        if (!synchronous || frame.version !== version) dirty = true;
      };
      try {
        const value = typeof plugin.render === "function" ? plugin.render(rows, columns) : "";
        track(value, (text) => { try { finish(text); } catch (error) { fail(error); } });
        synchronous = false;
      } catch (error) { frame.pending = false; pendingRender = false; throw error; }
    }
    return frame.text;
  }
  return (action, arg, origin) => {
    const previous = client;
    client = origin ?? null;
    try {
      if (failure) { const error = failure; failure = null; throw error; }
      if (action === "bind") {
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
