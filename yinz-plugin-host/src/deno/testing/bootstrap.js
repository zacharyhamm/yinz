// Installed only in the disposable test worker, before the SDK hides raw ops.
((ops) => {
  const dynamic = new Map();
  const permissions = [];
  let activePermission = null;
  let grantedPermissions = [];
  const dispatch = ops.op_yinz_command;
  function dispatchCommand(command, client, completion) {
    const name = typeof command === "string" ? command : Object.keys(command)[0];
    const result = dispatch(command, client, completion);
    return dynamic.has(name) ? {json: dynamic.get(name)(typeof command === "string" ? undefined : command[name], command)} : result;
  }
  function dispatchPermissions(immediate) {
    let failed = false, failure;
    while (!activePermission && permissions.length) {
      const request = permissions.shift();
      request.permissions = [...new Set([...grantedPermissions, ...request.permissions])];
      activePermission = request;
      try {
        dispatchCommand({RequestPluginPermissions: [...request.permissions]}, request.client, request.completion);
      } catch (error) {
        activePermission = null;
        if (request === immediate) { failed = true; failure = error; }
        else bridge("permissionResult", [request.completion, {Err: String(error)}], request.client);
      }
    }
    if (failed) throw failure;
  }
  ops.op_yinz_command = (command, client, completion) => {
    if (typeof command === "object" && Object.hasOwn(command, "RequestPluginPermissions") && completion !== null && completion !== undefined) {
      const request = {permissions: [...command.RequestPluginPermissions], client, completion};
      permissions.push(request);
      dispatchPermissions(request);
      return {json: null};
    }
    return dispatchCommand(command, client, completion);
  };
  function mock(replies) {
    const values = {};
    for (const [name, reply] of Object.entries(replies)) {
      if (typeof reply === "function") { dynamic.set(name, reply); values[name] = null; }
      else { dynamic.delete(name); values[name] = reply; }
    }
    call("mock", values);
  }
  const cases = [];
  let sealed = false, loaded = false, now = 0;
  const bridge = (...args) => globalThis.__yinzTestBridge(...args);
  const call = (op, value = null) => ops.op_yinz_test(op, value);
  const assert = (value, message = "assertion failed") => { if (!value) throw new Error(message); };
  function equal(a, b, seen = new Map()) {
    if (Object.is(a, b)) return true;
    if (a === null || b === null || typeof a !== "object" || typeof b !== "object") return false;
    const plain = value => [Object.prototype, null].includes(Object.getPrototypeOf(value));
    if (!(plain(a) && plain(b)) && Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;
    if (seen.has(a)) return seen.get(a) === b;
    seen.set(a, b);
    if (a instanceof ArrayBuffer) return equal(new Uint8Array(a), new Uint8Array(b), seen);
    if (a instanceof Date) return a.getTime() === b.getTime();
    if (a instanceof RegExp) return String(a) === String(b);
    if (a instanceof Map) return equal([...a], [...b], seen);
    if (a instanceof Set) return equal([...a], [...b], seen);
    const keys = Reflect.ownKeys(a);
    return keys.length === Reflect.ownKeys(b).length && keys.every(k => Object.hasOwn(b, k) && equal(a[k], b[k], seen));
  }
  function assertEquals(actual, expected, message) {
    if (!equal(actual, expected)) {
      const describe = value => { try { return JSON.stringify(value); } catch { return Object.prototype.toString.call(value); } };
      throw new Error(message ?? `expected ${describe(expected)}, got ${describe(actual)}`);
    }
  }
  function checkError(error, expected) {
    if (expected !== undefined) assert(String(error).includes(expected), `expected error containing ${expected}, got ${String(error)}`);
    return error;
  }
  function assertThrows(fn, expected) {
    try { fn(); } catch (e) { return checkError(e, expected); }
    throw new Error("expected function to throw");
  }
  async function assertRejects(fn, expected) {
    try { await fn(); } catch (e) { return checkError(e, expected); }
    throw new Error("expected promise to reject");
  }
  function test(name, options, fn) {
    if (typeof options === "function") { fn = options; options = {}; }
    if (sealed) throw new Error("tests must register during module initialization");
    if (typeof name !== "string" || !name.trim() || name.length > 1024 || typeof fn !== "function") throw new TypeError("test needs a name and function");
    if (cases.some(t => t.name === name)) throw new Error(`duplicate test: ${name}`);
    if (cases.length === 1000) throw new Error("at most 1000 tests per file");
    cases.push({name, options: {grants: [], ...options}, fn});
  }
  async function settle() { await ops.op_yinz_test_tick(); bridge("peek"); }
  async function loadPlugin(path, options = {}) {
    if (loaded) throw new Error("one plugin harness per test; use a separate test for another instance");
    loaded = true;
    mock(options.replies ?? {});
    const url = call("resolve", path);
    const plugin = await import(url);
    bridge("bind", plugin);
    bridge("load", options.configuration ?? {}, 1);
    await settle();
    return {
      stub(name, reply) { mock({[name]: reply}); },
      async event(event) {
        // Bind the answer before observers can enqueue more requests.
        const pending = Object.hasOwn(event, "PermissionRequestResult") ? activePermission : null;
        const answer = event.PermissionRequestResult;
        try { bridge("update", event, 1); }
        finally {
          if (pending) {
            if (answer === "Granted") grantedPermissions = pending.permissions;
            activePermission = null;
            bridge("permissionResult", [pending.completion, {Ok: answer}], pending.client);
            dispatchPermissions();
          }
        }
        await settle();
      },
      async pipe(message) { bridge("pipe", {source: {Plugin: 2}, args: {}, payload: null, is_private: true, ...message}, 1); await settle(); },
      async palette(message, clientId) {
        if (!Number.isInteger(clientId) || clientId < 0 || clientId > 0xffff) throw new TypeError("palette needs a valid client ID");
        if (message.context && message.context.clientId !== clientId) throw new TypeError("palette context clientId must match client ID");
        bridge("palette", message, clientId); await settle();
      },
      async advance(ms) {
        if (!Number.isFinite(ms) || ms < 0) throw new TypeError("advance needs nonnegative milliseconds");
        now += ms; bridge("update", {Timer: ms / 1000}, 1); await settle();
      },
      async render(rows = 24, columns = 80) {
        bridge("render", [rows, columns], 1); await settle();
        const text = bridge("render", [rows, columns], 1); return call("render", {text, rows, columns});
      },
      get commands() { return call("commands"); },
      get paletteCommands() { return call("commands").filter(command => command && Object.hasOwn(command, "CommandPalette")).map(command => command.CommandPalette); },
      get logs() { return call("logs"); },
      get renders() { return call("renders"); },
    };
  }
  const renderText = (text, rows = 24, columns = 80) => call("render", {text,rows,columns});
  const session = {
    fixture(files = {}, commands = {}) { return call("session:fixture", {files,commands}); },
    start(options = {}) { return call("session:start", options); },
    loadPlugin(path, options = {}) { return call("session:load", {path, ...options}); },
    key(key) { call("session:key", key); },
    text(text) { call("session:text", text); },
    mouse(event) { call("session:mouse", event); },
    resize(rows, columns) { call("session:resize", {rows, columns}); },
    action(kdl) { call("session:action", kdl); },
    async agent(request) {
      const id = call("session:agent", request);
      for (;;) {
        const result = call("session:agent-poll", id);
        if (result !== null) {
          if (Object.hasOwn(result, "Err")) throw new Error(result.Err);
          return result.Ok;
        }
        await ops.op_yinz_test_tick();
        call("session:poll");
      }
    },
    report(pane, kind, hook) { call("session:report", {pane, kind, hook}); },
    pipe(name, payload, options = {}) { return call("session:pipe", {name, payload, ...options}); },
    async rpc(plugin, name, payload = null, timeoutMs = 10000) {
      call("pending", `RPC ${plugin}/${name}`);
      const result = call("session:rpc", {plugin, name, payload, timeoutMs});
      call("pending", ""); return result;
    },
    stop(path, options = {}) { return call("session:stop", {path, ...options}); },
    status(path, options = {}) { return call("session:status", {path, ...options}); },
    reload(path, options = {}) { return call("session:reload", {path, ...options}); },
    get screen() { return call("session:screen"); },
    get events() { return call("session:events"); },
    get panes() { return call("session:panes"); },
    data(plugin, entity) { return call("session:data", {plugin, entity}); },
    catalog() { return call("session:catalog"); },
    get logs() { return call("session:logs"); },
    async waitFor(what, predicate, timeoutMs = 10000) {
      const end = call("clock") + timeoutMs;
      call("pending", what);
      while (!await predicate()) {
        if (call("clock") >= end) throw new Error(`timed out waiting for ${what}\n${session.screen}\n${session.logs}`);
        call("session:poll");
      }
      call("pending", "");
    },
    async waitForText(text, timeoutMs) { await this.waitFor(`screen containing ${text}`, () => this.screen.includes(text), timeoutMs); return this.screen; },
  };
  globalThis.__yinzTesting = Object.freeze({test, assert, assertEquals, assertThrows, assertRejects, loadPlugin, session, renderText});
  globalThis.__yinzTestRun = async (index, expected) => {
    sealed = true;
    if (index === null) return cases.map(({name, options}) => ({name, options}));
    assert(cases[index], "test registration changed between discovery and execution");
    assertEquals({name: cases[index].name, options: cases[index].options}, expected, "test registration changed between discovery and execution");
    await cases[index].fn();
    bridge("peek");
    return null;
  };
  Date.now = () => now;
})(Deno.core.ops);
