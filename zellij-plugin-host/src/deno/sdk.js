// Evaluated before guest modules. The returned bridge is held only by Rust.
(() => {
  "use strict";
  const core = Deno.core;
  const dispatch = core.ops.op_zellij_command;
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
  const permissionQueue = [];
  const grantedPermissions = new Set();
  const pending = new Set();
  const timers = new Set();
  let timerDeadline = null;
  function invalidate() { version++; dirty = true; }
  function fail(error) { failure ??= error instanceof Error ? error : new Error(String(error)); }
  function track(value, settled) {
    if (value !== null && (typeof value === "object" || typeof value === "function") && typeof value.then === "function") {
      const promise = Promise.resolve(value);
      pending.add(promise);
      promise.then(
        (result) => { pending.delete(promise); settled(result); },
        (error) => { pending.delete(promise); fail(error); },
      );
    } else settled(value);
  }
  function command(name, ...args) {
    if (client === null) throw new Error("host commands are unavailable during module initialization");
    const value = typeof name === "string"
      ? (args.length ? { [name]: args[0] } : name)
      : name;
    if (value !== null && typeof value === "object" && Object.keys(value).length === 1 && Object.hasOwn(value, "RequestPluginPermissions")) {
      enqueuePermissions(value.RequestPluginPermissions);
      return null;
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
  function waitForEvent(type, predicate = () => true) {
    return new Promise((resolve, reject) => {
      const off = on(type, (payload, event) => {
        try {
          if (predicate(payload, event)) { off(); resolve(payload); }
        } catch (error) { off(); reject(error); }
      });
      try { command("Subscribe", [type]); }
      catch (error) { off(); reject(error); }
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
      const off = on("PermissionRequestResult", (status) => {
        off();
        if (status === "Granted") {
          for (const permission of permissions) grantedPermissions.add(permission);
          request.resolve?.();
        } else request.reject?.(new Error("plugin permissions denied"));
        permissionQueue.shift();
        dispatchPermissions();
      });
      try {
        dispatch({ Subscribe: ["PermissionRequestResult"] }, request.client);
        dispatch({ RequestPluginPermissions: permissions }, request.client);
        return;
      } catch (error) {
        off();
        permissionQueue.shift();
        if (request.reject) request.reject(error);
        else if (request === immediate) throw error;
        else fail(error);
      }
    }
  }
  function armTimer() {
    let next = Infinity;
    for (const timer of timers) next = Math.min(next, timer.deadline);
    if (next !== Infinity && (timerDeadline === null || next < timerDeadline)) {
      command("SetTimeout", Math.max(0.001, (next - Date.now()) / 1000));
      timerDeadline = next;
    }
  }
  function sleep(milliseconds) {
    if (!Number.isFinite(milliseconds) || milliseconds < 0 || milliseconds > 2147483647) {
      return Promise.reject(new TypeError("sleep duration must be between 0 and 2147483647 milliseconds"));
    }
    return new Promise((resolve, reject) => {
      const timer = { deadline: Date.now() + milliseconds, resolve };
      try {
        command("Subscribe", ["Timer"]);
        timers.add(timer);
        armTimer();
      } catch (error) { timers.delete(timer); reject(error); }
    });
  }
  const api = Object.freeze({
    command,
    definePlugin: (value) => value,
    subscribe: (events) => command("Subscribe", events),
    unsubscribe: (events) => command("Unsubscribe", events.filter((event) => event !== "Timer" || timers.size === 0)),
    requestPermission: (permissions) => command("RequestPluginPermissions", permissions),
    requestRender: invalidate,
    on,
    sleep,
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
    // Starts are ordered by the executor. Promises are observed, never awaited here:
    // load may be waiting for the very permission event that update must deliver.
    if (typeof plugin[name] === "function") {
      track(plugin[name](arg), (result) => { if (result === true || name === "load") invalidate(); });
    }
    if (name === "update") {
      if (type === "Timer") {
        const now = Date.now();
        // Timer events have no ID. A stale/early one must still rearm the next
        // deadline; wall-clock rounding must never strand a sleep forever.
        timerDeadline = null;
        for (const timer of timers) {
          if (timer.deadline <= now) { timers.delete(timer); timer.resolve(); }
        }
        armTimer();
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
      } else if (action === "render") return render(...arg);
      else if (action === "peek") return dirty;
      else if (action === "status") { const requested = dirty; dirty = false; return requested; }
      else lifecycle(action, arg);
      return null;
    } finally { client = previous; }
  };
})()
