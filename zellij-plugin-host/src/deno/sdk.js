// Evaluated before guest modules. The returned bridge is held only by Rust.
(() => {
  "use strict";
  const core = Deno.core;
  const hostDispatch = core.ops.op_zellij_command;
  const validate = core.ops.op_zellij_validate;
  const log = core.ops.op_zellij_log;
  const workflowRemotePanes = core.ops.op_zellij_workflow_remote_panes;
  let metadata;
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
  // Core's console writes directly to the host's stdout. Route the familiar names to the
  // plugin's log instead.
  const logLine = (...values) => log(values.map(String).join("\t"));
  Object.defineProperty(globalThis, "console", {
    value: Object.freeze({ log: logLine, info: logLine, warn: logLine, error: logLine, debug: logLine, trace: logLine }),
    writable: false, configurable: false, enumerable: false,
  });

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
  let tracing = false;
  let nextTrace = 0n;
  function trace(enabled = true) {
    if (typeof enabled !== "boolean") throw new TypeError("trace requires a boolean");
    tracing = enabled;
  }
  function transcript(marker, origin = client) {
    if (!tracing) return;
    // Diagnostics must not replace a dispatch error or interrupt SDK cleanup.
    try { log(`trace instance=${rpcPrefix} client=${origin} ${marker}`); } catch (_) {}
  }
  function traceName(name) {
    return JSON.stringify(name.slice(0, 200)).replace(/[\u007f-\u009f\u2028\u2029]/g,
      c => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"));
  }
  function validateCommand(value, origin = client) {
    try { validate(value); }
    catch (error) { transcript("command invalid", origin); throw error; }
  }
  function dispatch(value, origin) {
    if (tracing) {
      validateCommand(value, origin);
      const name = typeof value === "string" ? value :
        value !== null && typeof value === "object" ? Object.keys(value)[0] : undefined;
      transcript(name === undefined ? "command invalid" : `command ${traceName(name)}`, origin);
    }
    return hostDispatch(value, origin);
  }
  function traceTask(label) {
    const id = ++nextTrace, origin = client;
    transcript(`${label} ${id} started`, origin);
    return outcome => transcript(`${label} ${id} ${outcome}`, origin);
  }
  function invalidate() { version++; dirty = true; }
  function fail(error) { failure ??= error instanceof Error ? error : new Error(String(error)); }
  function track(value, settled, rejected = fail) {
    if (value !== null && (typeof value === "object" || typeof value === "function") && typeof value.then === "function") {
      const promise = Promise.resolve(value);
      const terminal = traceTask("task");
      pending.add(promise);
      promise.then(
        (result) => { pending.delete(promise); terminal("finished"); settled(result); },
        (error) => { pending.delete(promise); terminal("failed"); rejected(error); },
      );
    } else settled(value);
  }
  function command(name, ...args) {
    if (client === null) throw new Error("host commands are unavailable during module initialization");
    let value = typeof name === "string"
      ? (args.length ? { [name]: args[0] } : name)
      : name;
    validateCommand(value);
    const key = typeof value === "string" ? value : value !== null && typeof value === "object" ? Object.keys(value)[0] : undefined;
    if (key === "PostMessageTo") {
      transcript(`command ${traceName(key)} refused`);
      throw new Error("PostMessageTo is not available from Deno: Deno plugins have no workers; use zellij.after/every, the async helpers, or a work queue");
    }
    if (Array.isArray(value?.RunCommand) && value.RunCommand.length === 5) {
      value = { ...value, RunCommand: [...value.RunCommand, null] };
    }
    if (value !== null && typeof value === "object" && Object.keys(value).length === 1 && Object.hasOwn(value, "RequestPluginPermissions")) {
      enqueuePermissions(value.RequestPluginPermissions);
      return null;
    }
    if (value !== null && typeof value === "object" && Object.keys(value).length === 1 && Object.hasOwn(value, "Unsubscribe") && Array.isArray(value.Unsubscribe)) {
      value = { Unsubscribe: value.Unsubscribe.filter((type) => !requiredEvents.has(type)) };
    }
    const response = dispatch(value, client);
    return Object.hasOwn(response, "bytes") ? bytes(response.bytes) : response.json;
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
    const terminal = traceTask("operation");
    const operation = { active: true, cleanup: () => {} };
    const promise = new Promise((resolve, reject) => {
      const settle = (finish, value, outcome) => {
        if (!operation.active) return;
        operation.active = false;
        operation.cleanup();
        terminal(outcome);
        finish(value);
      };
      operation.resolve = (value) => settle(resolve, value, "finished");
      operation.reject = (error, outcome = "failed") => settle(reject, error, outcome);
    });
    promise.cancel = () => {
      const error = new Error("operation cancelled");
      error.name = "AbortError";
      operation.reject(error, "cancelled");
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
  function requestResult(operation, name, args, contextIndex, project, onOutput) {
    const context = args[contextIndex];
    if (Object.hasOwn(context, "__zellij_async")) throw new TypeError("context key __zellij_async is reserved");
    const token = String(++nextAsync);
    context.__zellij_async = token;
    observeEvent(operation, `${name}Result`, (payload) => payload[3].__zellij_async === token, (payload) => {
      const context = { ...payload[3] };
      delete context.__zellij_async;
      return { ...project(payload), context };
    });
    if (onOutput) {
      const origin = client;
      const cleanup = operation.cleanup;
      let release = () => {};
      const off = on("RunCommandOutput", (payload) => {
        if (!operation.active || payload.context.__zellij_async !== token) return;
        const previous = client;
        client = origin;
        try {
          const context = { ...payload.context };
          delete context.__zellij_async;
          onOutput({ data: bytes(payload.data), stderr: payload.stderr, context });
        } catch (error) { operation.reject(error); }
        finally { client = previous; }
      });
      operation.cleanup = () => { cleanup(); off(); release(); };
      release = retainEvent("RunCommandOutput", origin);
    }
    command(name, args);
  }
  function swampRequest(options) {
    return cancellable((operation) => {
      const {directory, operation: name, payload = {}, id = `${rpcPrefix}-swamp-${++nextAsync}`, started = false} = record(options, "swampRequest options");
      let last;
      observeEvent(operation, "CustomMessage", ([message, encoded]) => {
        if (message !== "swamp-response") return false;
        const frame = JSON.parse(encoded);
        if (frame.id !== id) return false;
        if (frame.type === "managed.progress") return false;
        if (frame.type === "error") { last = {error: frame.error}; return true; }
        if (frame.type === "event") {
          last = frame.event;
          if (started && last.runId) { last = {runId: last.runId}; return true; }
          return false;
        }
        if (frame.type === "run.attached") {
          if (started) { last = {runId: frame.payload.runId}; return true; }
          return false;
        }
        if (frame.type === "done") { last ??= {}; return true; }
        if (frame.type === "run.interrupted" || frame.type === "run.elsewhere") {
          last = {error: {message: "Run stream is unavailable; reconcile persisted history"}}; return true;
        }
        last = frame.payload?.data ?? frame.payload ?? frame;
        return true;
      }, () => last);
      command("SwampRequest", {directory, id, operation: name, payload: JSON.stringify(payload)});
    });
  }
  function runCommand(options) {
    return cancellable((operation) => {
      const { command: argv, envVariables, cwd = "/host", context, maxBytesReturned, onOutput } = record(options, "runCommand options");
      if (!Array.isArray(argv) || !argv.length || argv.some((arg) => typeof arg !== "string") || argv[0] === "") {
        throw new TypeError("command must be a nonempty array of strings with an executable");
      }
      if (typeof cwd !== "string") throw new TypeError("cwd must be a string");
      if (maxBytesReturned !== undefined && (!Number.isSafeInteger(maxBytesReturned) || maxBytesReturned < 0)) {
        throw new TypeError("maxBytesReturned must be a nonnegative safe integer");
      }
      if (onOutput !== undefined && typeof onOutput !== "function") throw new TypeError("onOutput must be a function");
      requestResult(operation, "RunCommand", [
        [...argv], stringMap(envVariables, "envVariables"), cwd, stringMap(context, "context"), maxBytesReturned ?? null, onOutput ? true : null,
      ], 3, ([exitCode, stdout, stderr]) => ({ exitCode, stdout: new Uint8Array(stdout), stderr: new Uint8Array(stderr) }), onOutput);
    });
  }
  function paneStep(value) {
    const {name, stdin, timeoutMs, ...cmd} = record(value, 'pane step');
    if (cmd.path !== undefined && typeof cmd.path !== 'string') throw new TypeError('path must be a string');
    if (!cmd.path && cmd.remote_default_shell !== true) throw new TypeError('pane step needs a path or remote_default_shell');
    if (name !== undefined && typeof name !== 'string') throw new TypeError('step name must be a string');
    if (stdin !== undefined && typeof stdin !== 'string') throw new TypeError('stdin must be a string');
    if (timeoutMs !== undefined) validateTimerDuration(timeoutMs);
    validate(cmd);
    if (cmd.args !== undefined && (!Array.isArray(cmd.args) || Array.from(cmd.args).some(arg => typeof arg !== 'string'))) throw new TypeError('args must be strings');
    for (const key of ['cwd', 'remote_host', 'firecracker_vm', 'firecracker_image', 'firecracker_worktree']) if (cmd[key] != null && typeof cmd[key] !== 'string') throw new TypeError(`${key} must be a string or null`);
    if (cmd.remote_default_shell !== undefined && typeof cmd.remote_default_shell !== 'boolean') throw new TypeError('remote_default_shell must be a boolean');
    if (cmd.target != null && cmd.target !== 'Local') {
      const target = record(cmd.target, 'target');
      if (Object.keys(target).length !== 1 || (!target.Ssh && !target.Firecracker)) throw new TypeError('invalid pane target');
      const fields = record(target.Ssh ?? target.Firecracker, 'target fields');
      if (typeof fields[target.Ssh ? 'host' : 'vm'] !== 'string') throw new TypeError('target needs a host or vm');
      for (const key of ['image', 'worktree']) if (fields[key] != null && typeof fields[key] !== 'string') throw new TypeError(`target ${key} must be a string or null`);
    }
    if (cmd.target == null) {
      if (cmd.remote_host?.trim() && cmd.firecracker_vm?.trim()) throw new TypeError('remote_host and firecracker_vm conflict');
      if (!cmd.firecracker_vm?.trim() && (cmd.firecracker_image?.trim() || cmd.firecracker_worktree)) throw new TypeError('image and worktree need firecracker_vm');
      if (cmd.firecracker_vm?.trim()) {
        for (const name of [cmd.firecracker_vm.trim(), cmd.firecracker_image?.trim()].filter(Boolean)) if (!/^[a-zA-Z0-9._-]{1,64}$/.test(name)) throw new TypeError('invalid Firecracker name');
        if (cmd.firecracker_worktree && !cmd.firecracker_worktree.startsWith('/')) throw new TypeError('firecracker_worktree must be absolute');
      }
    }
    // Copy nested command data so a caller cannot change a validated queued step.
    return {...JSON.parse(JSON.stringify(cmd)), name, stdin, timeoutMs};
  }
  function paneTarget(cmd) {
    const target = cmd.target ?? (cmd.firecracker_vm?.trim() ? {Firecracker: {vm: cmd.firecracker_vm.trim(), image: cmd.firecracker_image?.trim() || null, worktree: cmd.firecracker_worktree || null}} : cmd.remote_host?.trim() ? {Ssh: {host: cmd.remote_host.trim()}} : 'Local');
    return target === 'Local' ? ['Local'] : target.Ssh ? ['Ssh', target.Ssh.host] : ['Firecracker', target.Firecracker.vm, target.Firecracker.image ?? null, target.Firecracker.worktree ?? null];
  }
  function paneCommand(step, pane) {
    const {name, stdin, timeoutMs, ...cmd} = step;
    if (cmd.cwd === 'inherit') {
      const info = pane && command('GetPaneInfo', pane);
      cmd.cwd = info && JSON.stringify(paneTarget(info)) === JSON.stringify(paneTarget(cmd)) ? info.cwd : null;
    }
    return cmd;
  }
  // One observer owns both startup and exit, including events queued during pane creation.
  function observePane(operation, pane, context, started = () => {}) {
    const origin = client, cleanup = [], queued = [];
    let id = pane, hasStarted = false;
    operation.cleanup = () => { for (const dispose of cleanup.splice(0)) dispose(); };
    const receive = (type, payload) => {
      if (!operation.active) return;
      if (id === null) { queued.push([type, payload]); return; }
      const previous = client; client = origin;
      try {
        if (type === 'PaneClosed') {
          if (payload.Terminal === id) operation.resolve({paneId: {Terminal:id}, exitCode:null, closed:true});
        } else if (payload[0] === id && (!context || payload.at(-1).__zellij_pane_step === context.__zellij_pane_step)) {
          if (type === 'CommandPaneExited') operation.resolve({paneId:{Terminal:id}, exitCode:payload[1], closed:false});
          else if (!hasStarted) { hasStarted = true; started(); }
        }
      } catch (error) { operation.reject(error); }
      finally { client = previous; }
    };
    for (const type of ['CommandPaneOpened', 'CommandPaneReRun', 'CommandPaneExited', 'PaneClosed']) {
      cleanup.push(on(type, payload => receive(type, payload)));
      cleanup.push(retainEvent(type));
    }
    return pane => { id = terminalId(pane); for (const event of queued.splice(0)) receive(...event); };
  }
  function waitForPaneExit(pane) {
    return cancellable(operation => {
      const id = terminalId(pane);
      observePane(operation, id, null);
      const dispose = operation.cleanup;
      const exited = info => info?.id === id && !info.is_plugin && info.exited === true;
      const result = info => ({paneId:{Terminal:id}, exitCode:info.exit_status, closed:false});
      try {
        observeEvent(operation, 'PaneUpdate', () => exited(agentPanes.get(id)), () => result(agentPanes.get(id)));
      } finally {
        const disposeMetadata = operation.cleanup;
        operation.cleanup = () => { dispose(); disposeMetadata(); };
      }
      const info = command('GetPaneInfo', {Terminal:id});
      if (info === null) operation.resolve({paneId:{Terminal:id}, exitCode:null, closed:true});
      else if (exited(info)) operation.resolve(result(info));
    });
  }
  function runInPane(pane, value) {
    return cancellable(operation => {
      const id = terminalId(pane), step = paneStep(value);
      const context = {__zellij_pane_step: `${rpcPrefix}:${++nextAsync}`};
      let timer;
      observePane(operation, id, context, () => {
        if (step.stdin !== undefined) command('WriteCharsToPaneId', [step.stdin, {Terminal:id}]);
        if (step.timeoutMs !== undefined) timer = addTimer(step.timeoutMs, () => operation.reject(new Error('pane wait timed out')), operation.reject);
      });
      const dispose = operation.cleanup;
      operation.cleanup = () => { dispose(); timer?.cancel(); };
      command('RunCommandInPane', {pane_id:{Terminal:id}, command:paneCommand(step, {Terminal:id}), context, keep_scrollback:true});
    });
  }
  function paneSequence(value) {
    const opts = {...record(value, 'paneSequence options')};
    if (!Array.isArray(opts.steps) || !opts.steps.length) throw new TypeError('paneSequence needs nonempty steps');
    const steps = Array.from(opts.steps, paneStep);
    for (const key of ['stopOnFailure', 'closeOnSuccess', 'keepScrollback']) if (opts[key] !== undefined && typeof opts[key] !== 'boolean') throw new TypeError(`${key} must be a boolean`);
    for (const key of ['onStep', 'onDone']) if (opts[key] !== undefined && typeof opts[key] !== 'function') throw new TypeError(`${key} must be a function`);
    if (opts.name !== undefined && typeof opts.name !== 'string') throw new TypeError('name must be a string');
    if (opts.onCancel !== undefined && !['close', 'keep'].includes(opts.onCancel)) throw new TypeError('onCancel must be close or keep');
    if (opts.floating !== undefined && typeof opts.floating !== 'boolean') record(opts.floating, 'floating coordinates');
    const grace = opts.killGraceMs ?? 5000;
    validateTimerDuration(grace);
    if (opts.floating && opts.floating !== true) {
      validate(opts.floating);
      opts.floating = {...opts.floating};
      for (const key of ['pinned', 'borderless']) if (opts.floating[key] != null && typeof opts.floating[key] !== 'boolean') throw new TypeError(`${key} must be a boolean`);
      for (const key of ['x', 'y', 'width', 'height']) {
        const v = opts.floating[key];
        if (v == null) continue;
        record(v, key);
        const names = Object.keys(v), n = v[names[0]];
        if (names.length !== 1 || !['Percent', 'Fixed'].includes(names[0]) || (typeof n !== 'bigint' && !Number.isSafeInteger(n)) || n < 0) throw new TypeError(`invalid floating ${key}`);
        opts.floating[key] = {...v};
      }
    }
    let paneId = null, status = 'running', index = 0, failedStep = null, current, timer, ending = null;
    const exitCodes = [];
    const report = error => log(String(error?.stack ?? error));
    const callback = (fn, ...args) => { try { if (fn) track(fn(...args), () => {}, report); } catch (error) { report(error); } };
    let resolve, reject;
    const done = new Promise((yes, no) => { resolve = yes; reject = no; });
    const clean = () => { if (current) {current.active = false; current.cleanup();} timer?.cancel(); timer = null; };
    const failure = error => { clean(); status = 'failed'; reject(error); };
    const finish = outcome => {
      clean(); status = outcome;
      try {
        if ((outcome === 'success' && opts.closeOnSuccess) || (outcome === 'cancelled' && opts.onCancel === 'close')) command('CloseTerminalPane', paneId.Terminal);
        const result = {status, paneId, exitCodes:[...exitCodes], failedStep};
        callback(opts.onDone, result); resolve(result);
      } catch (error) { failure(error); }
    };
    const exited = result => {
      clean();
      if (result.closed) { finish('pane_closed'); return; }
      exitCodes.push(result.exitCode);
      if ((!ending && result.exitCode !== 0) || ending === 'failed') failedStep ??= index + 1;
      callback(opts.onStep, index + 1, {...steps[index]}, result.exitCode);
      if (ending) finish(ending);
      else if (failedStep && opts.stopOnFailure !== false) finish('failed');
      else if (++index === steps.length) finish(failedStep ? 'failed' : 'success');
      else launch();
    };
    const escalate = outcome => {
      if (status !== 'running' || ending) return;
      // A refused initial cancellation must not change the running sequence.
      command('SendSigintToPaneId', paneId);
      ending = outcome; status = outcome === 'cancelled' ? 'cancelling' : 'running';
      timer?.cancel();
      try {
        timer = addTimer(grace, () => {
          try {
            command('SendSigkillToPaneId', paneId);
            timer?.cancel();
            timer = addTimer(grace, () => exited({exitCode:null, closed:false}), failure);
          } catch (error) { failure(error); }
        }, failure);
      } catch (error) { failure(error); }
    };
    function launch() {
      try {
        const step = steps[index], context = {__zellij_pane_step: `${rpcPrefix}:${++nextAsync}`};
        current = {active:true, cleanup:() => {}, resolve:exited, reject:failure};
        const setPane = observePane(current, paneId?.Terminal ?? null, context, () => {
          if (step.stdin !== undefined) command('WriteCharsToPaneId', [step.stdin, paneId]);
          if (step.timeoutMs !== undefined && !ending) timer = addTimer(step.timeoutMs, () => {try {escalate('failed');} catch (error) {failure(error);}}, failure);
        });
        const cmd = paneCommand(step, paneId);
        if (paneId) command('RunCommandInPane', {pane_id:paneId, command:cmd, context, keep_scrollback:opts.keepScrollback !== false});
        else {
          paneId = opts.floating ? command('OpenCommandPaneFloating', [cmd, opts.floating === true ? null : opts.floating, context]) : command('OpenCommandPane', [cmd, context]);
          if (!paneId) throw new Error('No pane was returned');
          if (opts.name !== undefined) command('RenameTerminalPane', [terminalId(paneId), opts.name]);
          setPane(paneId);
        }
      } catch (error) { failure(error); }
    }
    const handle = {get paneId() {return paneId;}, get status() {return status;}, done,
      push(step) {if (status === 'running' && !ending) steps.push(paneStep(step));},
      cancel() {escalate('cancelled');}};
    launch();
    return handle;
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
    // The revision guard that says "this row must not exist yet", for a put that creates.
    REV_ABSENT: 0,
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
  const RPC_MAX_BYTES = 1048576;
  function jsonMessage(value, maxBytes = 64000) {
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
    if (encodeUtf8(encoded).length > maxBytes) throw new RangeError(`RPC/Hocus payload exceeds ${maxBytes} bytes`);
    return encoded;
  }
  function sendPluginMessage(name, payload, destination = null, pluginUrl = null, pluginConfig = {}, maxBytes = 64000) {
    const encoded = jsonMessage(payload, maxBytes);
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
        const maxBytes = name.startsWith("hocus_tool:") ? 64000 : RPC_MAX_BYTES;
        rpcCalls.set(callId, {operation, pluginId, maxBytes});
        if (timeoutMs !== undefined) timer = addTimer(timeoutMs, () => {
          const error = new Error("RPC timed out: " + name);
          error.name = "TimeoutError";
          operation.reject(error);
        }, operation.reject);
        sendPluginMessage("zellij_rpc_call", {version: 1, call_id: callId, name, payload}, pluginId ?? null, pluginUrl ?? null, config, maxBytes);
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
    if (!Number.isSafeInteger(from) || from < 0 || from > 4294967295 || typeof message.payload !== "string" || encodeUtf8(message.payload).length > RPC_MAX_BYTES) return true;
    let call;
    try { call = JSON.parse(message.payload); validate(call); } catch (_) { return true; }
    if (call?.version !== 1 || typeof call.call_id !== "string" || !call.call_id) return true;
    if (message.name === "zellij_rpc_reply") {
      const waiting = rpcCalls.get(call.call_id);
      if (!waiting || (waiting.pluginId !== undefined && waiting.pluginId !== from) || encodeUtf8(message.payload).length > waiting.maxBytes) return true;
      const hasError = Object.hasOwn(call, "error"), hasResult = Object.hasOwn(call, "result");
      if (hasError && (hasResult || typeof call.error !== "string")) return true;
      // Legacy RPC senders can omit the result field.
      if (hasError) waiting.operation.reject(new Error(call.error));
      else waiting.operation.resolve(call.result ?? null);
      return true;
    }
    if (typeof call.name !== "string" || !call.name) return true;
    const maxBytes = call.name.startsWith("hocus_tool:") ? 64000 : RPC_MAX_BYTES;
    if (encodeUtf8(message.payload).length > maxBytes) return true;
    const origin = client;
    const reply = (result, error) => {
      const previous = client;
      client = origin;
      try {
        let payload;
        try {
          payload = {version: 1, call_id: call.call_id, ...(error === undefined ? {result: result === undefined ? null : result} : {error: String(error).slice(0, 2000)})};
          jsonMessage(payload, maxBytes);
        } catch (error) {
          payload = {version: 1, call_id: call.call_id, error: String(error).slice(0, 2000)};
        }
        // Leave room for the opaque caller ID even when it nearly fills the envelope.
        if (Object.hasOwn(payload, "error")) {
          while (encodeUtf8(JSON.stringify(payload)).length > maxBytes && payload.error.length) payload.error = payload.error.slice(0, Math.floor(payload.error.length / 2));
        }
        sendPluginMessage("zellij_rpc_reply", payload, from, null, {}, maxBytes);
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
    sanitizeBytes: (value) => pure('Sanitize', decodeUtf8(bytes(value))),
  });
  // Every PluginCommand, first letter lowercased. The inventory is checked against the Rust enum.
  const HOST_COMMANDS = "SwampRequest CheckPlugin Subscribe Unsubscribe SetSelectable ShowCursor GetPluginIds GetZellijVersion OpenFile OpenFileFloating OpenTerminal OpenTerminalFloating OpenCommandPane OpenCommandPaneFloating OpenCommandPaneOverlay SwitchTabTo SetTimeout ExecCmd PostMessageTo PostMessageToPlugin HideSelf ShowSelf SwitchToMode NewTabsWithLayout NewTab NewTabUnfocused NewTiledPaneInTab ToggleFloatingPanes NewPane GoToNextTab GoToPreviousTab Resize ResizeWithDirection FocusNextPane FocusPreviousPane FocusLastPane MoveFocus MoveFocusOrTab Detach EditScrollback Write WriteChars ToggleTab MovePane MovePaneWithDirection ClearScreen ScrollUp ScrollDown ScrollToTop ScrollToBottom PageScrollUp PageScrollDown ToggleFocusFullscreen ToggleFocusNoUiFullscreen TogglePaneFrames SetPaneFrameStyle TogglePaneEmbedOrEject UndoRenamePane CloseFocus ToggleActiveTabSync CloseFocusedTab UndoRenameTab QuitZellij PreviousSwapLayout NextSwapLayout GoToTabName FocusOrCreateTab GoToTab StartOrReloadPlugin CloseTerminalPane ClosePluginPane FocusTerminalPane FocusPluginPane RenameTerminalPane RenamePluginPane RenameTab ReportPanic RequestPluginPermissions SwitchSession DeleteDeadSession DeleteAllDeadSessions OpenTerminalInPlace OpenFileInPlace OpenCommandPaneInPlace SubscribePaneOutput RunCommand WebRequest CancelWebRequest RenameSession Notify UnblockCliPipeInput BlockCliPipeInput CliPipeOutput MessageToPlugin DisconnectOtherClients KillSessions ScanHostFolder WatchFilesystem FsRead FsList FsMetadata FsWrite FsRemove FsCreateDir FsRename GrantPluginPermissions DumpSessionLayout CloseSelf NewTabsWithLayoutInfo Reconfigure HidePaneWithId ShowPaneWithId OpenCommandPaneBackground RerunCommandPane RunCommandInPane ResizePaneIdWithDirection EditScrollbackForPaneWithId GetPaneScrollback WriteToPaneId WriteCharsToPaneId SendSigintToPaneId SendSigkillToPaneId GetPanePid GetPaneRunningCommand GetPaneCwd GetPaneGitProbe MovePaneWithPaneId MovePaneWithPaneIdInDirection ClearScreenForPaneId ScrollUpInPaneId ScrollDownInPaneId ScrollToTopInPaneId ScrollToBottomInPaneId PageScrollUpInPaneId PageScrollDownInPaneId TogglePaneIdFullscreen TogglePaneEmbedOrEjectForPaneId CloseTabWithIndex BreakPanesToNewTab BreakPanesToTabWithIndex SwitchTabToId GoToTabWithId CloseTabWithId RenameTabWithId ArrangeProjectPanes SetTabProjects BreakPanesToTabWithId ReloadPlugin LoadNewPlugin RebindKeys RegisterDefaultKeybindings ListClients ChangeHostFolder SetFloatingPanePinned StackPanes ChangeFloatingPanesCoordinates ChangeOverlayCoordinates TogglePaneBorderless SetPaneBorderless OpenCommandPaneNearPlugin OpenCommandPaneInGitCheckoutNearPlugin OpenTerminalNearPlugin OpenTerminalFloatingNearPlugin OpenTerminalInPlaceOfPlugin OpenCommandPaneFloatingNearPlugin OpenCommandPaneInPlaceOfPlugin OpenFileNearPlugin OpenFileFloatingNearPlugin OpenFileInPlaceOfPlugin GroupAndUngroupPanes HighlightAndUnhighlightPanes SetRelatedPanes RevealPaneWithId CloseMultiplePanes FloatMultiplePanes EmbedMultiplePanes SetSelfMouseSelectionSupport InterceptKeyPresses ClearKeyPressesIntercepts ReplacePaneWithExistingPane RunAction CopyToClipboard OverrideLayout SaveLayout DeleteLayout RenameLayout EditLayout GenerateRandomName DumpLayout ParseLayout GetLayoutDir GetUserPluginDir GetFocusedPaneInfo SaveSession CurrentSessionLastSavedTime GetPaneInfo GetTabInfo GetSessionEnvironmentVariables GetPluginAliasConfiguration OpenCommandPaneInNewTab OpenPluginPaneInNewTab OpenEditorPaneInNewTab OpenCommandPaneInPlaceOfPaneId OpenTerminalPaneInPlaceOfPaneId OpenEditPaneInPlaceOfPaneId HideFloatingPanes ShowFloatingPanes SetPaneColor SetPaneRegexHighlights SetContextMenu ClearPaneHighlights OpenPluginPaneFloating GetSessionList KillSessionsAndReply DeleteDeadSessionAndReply DeleteAllDeadSessionsAndReply FocusHostSession ListRecordings ListNotifications SearchRecordings WarpPaneToSession WarpPaneFromSession WarpTabToSession WarpTabFromSession ListOrphanedPanes AdoptOrphanedPane DataDefineEntity DataApply DataFetch DataCount DataWatch DataUnwatch DataCatalog SetPaneTitlePrefix SetPaneSubtitle EnqueueQueueJob ConfigureQueue CancelQueueJob QueueStatus FinishQueueJob SetPaneAgent SetPaneAgentRun ProjectConfigRead ProjectConfigWrite SpawnAgent SetQuitNotice ReloadPluginByUrl ReadPluginLog GetLocalUtcOffset".split(" ");
  // Tuple commands that also accept one object of named arguments. Omitted fields become null,
  // which keeps the arity serde counts and reads as absent for an Option slot.
  const tupleFields = {
    RenameTab: ["tabIndex", "name"], RenameTabWithId: ["tabId", "name"],
    RenameTerminalPane: ["paneId", "name"], RenamePluginPane: ["paneId", "name"],
    FocusTerminalPane: ["paneId", "shouldFloatIfHidden", "shouldBeInPlaceIfHidden"],
    FocusPluginPane: ["paneId", "shouldFloatIfHidden", "shouldBeInPlaceIfHidden"],
    ShowPaneWithId: ["paneId", "shouldFloatIfHidden", "shouldFocusPane"],
    SetFloatingPanePinned: ["paneId", "shouldBePinned"], SetPaneBorderless: ["paneId", "borderless"],
    RevealPaneWithId: ["paneId", "label"], Notify: ["message", "paneId", "click"],
    CliPipeOutput: ["pipeName", "output"], Reconfigure: ["configuration", "writeToDisk"],
    RunCommand: ["command", "envVariables", "cwd", "context", "maxBytesReturned", "streamOutput"],
    OpenCommandPaneInGitCheckoutNearPlugin: ["command", "sourceDirectory", "checkout"],
    WebRequest: ["url", "verb", "headers", "body", "context", "streamId", "proxy"],
  };
  // The required slots an omitted named field fills, where null is not a value Rust accepts.
  const tupleDefaults = { WebRequest: { verb: "Get", headers: {}, body: [], context: {} } };
  function positional(name, arg) {
    const fields = tupleFields[name];
    if (!fields || arg === null || typeof arg !== "object" || Array.isArray(arg) || arg instanceof Uint8Array) return arg;
    validate(arg);
    record(arg, `${name} arguments`);
    for (const field of Object.keys(arg)) if (!fields.includes(field)) throw new TypeError(`unknown ${name} argument: ${field}`);
    const defaults = tupleDefaults[name] ?? {};
    const out = fields.map((field) => arg[field] === undefined ? (defaults[field] ?? null) : arg[field]);
    if (name === "WebRequest" && typeof out[3] === "string") out[3] = bytes(out[3]);
    return out;
  }
  const host = Object.freeze(Object.fromEntries(HOST_COMMANDS.map((name) => [
    name[0].toLowerCase() + name.slice(1),
    (...args) => args.length === 0 ? command(name) : args.length === 1 ? command(name, positional(name, args[0])) : command(name, args),
  ])));
  function bytes(value) {
    if (value instanceof Uint8Array) return value;
    if (typeof value === "string") return encodeUtf8(value);
    if (!Array.isArray(value) || value.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)) {
      throw new TypeError("bytes requires a Uint8Array, a string, or an array of integers 0-255");
    }
    return Uint8Array.from(value);
  }
  function keyName(bareKey) {
    if (typeof bareKey === "string") return bareKey;
    validate(bareKey);
    if (bareKey === null || typeof bareKey !== "object" || Object.keys(bareKey).length !== 1) throw new TypeError("keyName requires a BareKey");
    const [variant, value] = Object.entries(bareKey)[0];
    return variant === "Char" ? String(value) : variant + String(value);
  }
  function paneKey(pane) {
    if (typeof pane === "number") return `terminal_${terminalId(pane)}`;
    validate(pane);
    if (pane === null || typeof pane !== "object" || Object.keys(pane).length !== 1) throw new TypeError("paneKey requires a PaneId");
    const [variant, id] = Object.entries(pane)[0];
    if (!["Terminal", "Plugin"].includes(variant) || !Number.isSafeInteger(id) || id < 0 || id > 4294967295) throw new TypeError("paneKey requires a PaneId");
    return `${variant.toLowerCase()}_${id}`;
  }
  function mouse(raw) {
    validate(raw);
    const payload = raw !== null && typeof raw === "object" && Object.keys(raw).length === 1 && Object.hasOwn(raw, "Mouse") ? raw.Mouse : raw;
    if (payload === null || typeof payload !== "object" || Object.keys(payload).length !== 1) throw new TypeError("mouse requires a Mouse event");
    const [kind, value] = Object.entries(payload)[0];
    if (Array.isArray(value) && value.length === 2) return { kind, line: value[0], col: value[1] };
    if (typeof value === "number" || typeof value === "bigint") return { kind, lines: value };
    throw new TypeError("mouse requires a Mouse event");
  }
  // The positional Rust tuple variants get camelCase fields. Other events retain `value`.
  const TUPLE_EVENT_FIELDS = Object.freeze({
    RunCommandResult: ["exitCode", "stdout", "stderr", "context"],
    WebRequestResult: ["status", "headers", "body", "context"],
    SessionUpdate: ["sessions", "resurrectableSessions"],
    CustomMessage: ["message", "payload"],
    CommandPaneOpened: ["paneId", "context"],
    CommandPaneExited: ["paneId", "exitCode", "context"],
    CommandPaneReRun: ["paneId", "context"],
    EditPaneOpened: ["paneId", "context"],
    EditPaneExited: ["paneId", "exitCode", "context"],
    UserAction: ["action", "clientId", "terminalId", "cliClientId"],
    ActionComplete: ["action", "paneId", "context"],
    CwdChanged: ["paneId", "cwd", "focusedClientIds"],
    CommandChanged: ["paneId", "command", "isForeground", "focusedClientIds"],
    AvailableLayoutInfo: ["layouts", "errors"],
  });
  const BYTE_EVENT_FIELDS = Object.freeze({ RunCommandResult: ["stdout", "stderr"], WebRequestResult: ["body"] });
  // A raw event as a discriminated union. `value` is the payload itself, untouched, so nothing a
  // plugin already does with the raw shape changes; the named fields are the convenience.
  function decodeEvent(raw) {
    validate(raw);
    if (typeof raw === "string") return { type: raw, value: undefined, raw };
    if (raw === null || typeof raw !== "object" || Array.isArray(raw) || Object.keys(raw).length !== 1) {
      throw new TypeError("event requires an Event: a unit event name or one {Type: payload} object");
    }
    const [type, value] = Object.entries(raw)[0];
    const decoded = { type, value, raw };
    const fields = TUPLE_EVENT_FIELDS[type];
    if (fields) {
      if (!Array.isArray(value)) throw new TypeError(`${type} carries a tuple payload`);
      fields.forEach((name, index) => { decoded[name] = value[index] === undefined ? null : value[index]; });
      for (const name of BYTE_EVENT_FIELDS[type] ?? []) decoded[name] = bytes(decoded[name]);
    } else if (type === "RunCommandOutput") {
      Object.assign(decoded, { data: bytes(value.data), stderr: value.stderr, context: value.context });
    } else if (type === "PaneOutput") {
      Object.assign(decoded, { paneId: value.pane_id, data: bytes(value.data), context: value.context });
    } else if (type === "WebRequestStreamChunk") {
      Object.assign(decoded, { streamId: value.stream_id, chunk: bytes(value.chunk), done: value.done, error: value.error });
    } else if (type === "Key" || type === "InterceptedKeyPress") {
      decoded.key = keyName(value.bare_key);
    } else if (type === "Mouse") {
      Object.assign(decoded, mouse(value));
    } else if (type.startsWith("FileSystem") && Array.isArray(value)) {
      decoded.files = value.map(([path, metadata]) => ({ path, metadata }));
    } else if (type === "PluginDataChanged") {
      decoded.records = toRecords(value.rows);
    }
    return decoded;
  }
  function onEvent(type, handler) {
    if (typeof handler !== "function") throw new TypeError("event handler must be a function");
    return on(type, (_payload, raw) => handler(decodeEvent(raw)));
  }
  const KEYBINDING_PIPE = "__zellij_default_keybinding:";
  let keybindingHandlers = new Map();
  function kdlQuote(value) {
    if (typeof value === "boolean" || (typeof value === "number" && Number.isFinite(value))) return String(value);
    if (typeof value !== "string") throw new TypeError("KDL values must be strings, finite numbers, or booleans");
    return '"' + value.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n").replace(/\r/g, "\\r").replace(/\t/g, "\\t") + '"';
  }
  function kdlName(value) {
    return /^[A-Za-z_][A-Za-z0-9_-]*$/.test(value) ? value : kdlQuote(value);
  }
  function kdlEntries(values) {
    return Object.keys(values).sort().map((key) => `${kdlName(key)} ${kdlQuote(values[key])}`);
  }
  function action(name, ...args) {
    if (typeof name !== "string" || !name) throw new TypeError("an action needs a name");
    if (args.length === 1 && args[0] !== null && typeof args[0] === "object" && !Array.isArray(args[0])) {
      validate(args[0]);
      const spec = record(args[0], "action");
      if (["args", "props", "children"].some((key) => Object.hasOwn(spec, key))) {
        for (const key of Object.keys(spec)) if (!["args", "props", "children"].includes(key)) throw new TypeError(`unknown action field: ${key}`);
        return Object.freeze({ name, args: [...(spec.args ?? [])], props: { ...(spec.props ?? {}) }, ...(spec.children ? { children: { ...spec.children } } : {}) });
      }
      return Object.freeze({ name, args: [], props: { ...spec } });
    }
    return Object.freeze({ name, args: [...args], props: {} });
  }
  function actionKdl(value) {
    if (typeof value === "string") return kdlName(value);
    validate(value);
    const spec = record(value, "action");
    if (typeof spec.name !== "string" || !spec.name) throw new TypeError("an action needs a name");
    const out = [kdlName(spec.name)];
    for (const arg of spec.args ?? []) out.push(kdlQuote(arg));
    for (const key of Object.keys(spec.props ?? {}).sort()) out.push(`${kdlName(key)}=${kdlQuote(spec.props[key])}`);
    if (spec.children) out.push(`{ ${kdlEntries(spec.children).join("; ")}; }`);
    return out.join(" ");
  }
  function registerDefaultKeybindings(bindings) {
    if (!Array.isArray(bindings)) throw new TypeError("default keybindings must be an array");
    const replacement = new Map();
    const modes = [];
    bindings.forEach((binding, position) => {
      const index = position + 1;
      // A handler is a function, which the transport validator rejects; check the fields instead.
      record(binding, `default keybinding ${index}`);
      for (const key of Object.keys(binding)) if (!["mode", "key", "actions", "handler"].includes(key)) throw new TypeError(`unknown default keybinding field: ${key}`);
      const hasActions = binding.actions !== undefined, hasHandler = typeof binding.handler === "function";
      if (hasActions === hasHandler) throw new TypeError(`default keybinding ${index} needs exactly one of actions or handler`);
      if (typeof binding.mode !== "string" || !binding.mode) throw new TypeError(`default keybinding ${index} needs a mode`);
      if (typeof binding.key !== "string" || !binding.key) throw new TypeError(`default keybinding ${index} needs a key`);
      let actions;
      if (hasHandler) {
        const name = KEYBINDING_PIPE + index;
        replacement.set(name, binding.handler);
        actions = [`MessagePlugin { name ${kdlQuote(name)}; }`];
      } else {
        validate(binding.actions);
        if (!Array.isArray(binding.actions)) throw new TypeError(`default keybinding ${index} actions must be an array`);
        actions = binding.actions.map(actionKdl);
      }
      modes.push(`${kdlName(binding.mode)} { bind ${kdlQuote(binding.key)} { ${actions.join("; ")}; }; }`);
    });
    command("RegisterDefaultKeybindings", `keybinds clear-defaults=true { ${modes.join("; ")}${modes.length ? "; " : ""}}`);
    keybindingHandlers = replacement;
  }
  function keybindingPipe(message) {
    if (typeof message.name !== "string" || !message.name.startsWith(KEYBINDING_PIPE)) return false;
    const handler = message.source === "Keybind" ? keybindingHandlers.get(message.name) : undefined;
    if (handler) track(handler(message), (result) => { if (result === true) invalidate(); });
    return true;
  }
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
      toString: () => w.render(),
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
  // A raw event may be handed to a widget as it arrived: {Key: key} and {Mouse: mouse} unwrap,
  // a bare payload passes through.
  function unwrapEvent(value, tags) {
    if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
      const keys = Object.keys(value);
      if (keys.length === 1 && tags.includes(keys[0])) return value[keys[0]];
    }
    return value;
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
      handleKey: (key) => call('Key', unwrapEvent(key, ['Key', 'InterceptedKeyPress'])),
      handleMouse: (mouse) => call('Mouse', unwrapEvent(mouse, ['Mouse'])),
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
    markdown(source, {compact = false, dim = false} = {}) {
      const state = core.ops.op_zellij_ui_create({kind:'Markdown',source,compact,dim});
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

  const api = {
    get serverBinary() { return metadata.serverBinary; },
    get serverSession() { return metadata.serverSession; },
    get serverIncarnation() { return metadata.serverIncarnation; },
    get swampTiming() { return metadata.swampTiming === true; },
    monotonicMs: () => core.ops.op_zellij_monotonic_ms(),
    get instanceId() { return rpcPrefix; },
    command,
    trace,
    definePlugin: (value) => value,
    subscribe: (events) => command("Subscribe", events),
    unsubscribe: (events) => command("Unsubscribe", events),
    requestPermission: (permissions) => command("RequestPluginPermissions", permissions),
    requestRender: invalidate,
    on,
    onEvent,
    event: decodeEvent,
    sleep,
    after: (milliseconds, callback) => callbackTimer(milliseconds, callback, false),
    every: (milliseconds, callback) => callbackTimer(milliseconds, callback, true),
    runCommand,
    swampRequest,
    paneSequence,
    waitForPaneExit,
    runInPane,
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
    host,
    bytes,
    keyName,
    paneKey,
    mouse,
    /** The active tab, as the Wasm shim's get_focused_tab answers it. */
    getFocusedTab: (tabs) => {
      if (!Array.isArray(tabs)) throw new TypeError("getFocusedTab requires the TabUpdate payload");
      return tabs.find((tab) => tab !== null && typeof tab === "object" && tab.active === true) ?? null;
    },
    /** The focused terminal pane of one tab, plugins ignored, as the Wasm shim's get_focused_pane answers it. */
    getFocusedPane: (tabPosition, manifest) => {
      if (!Number.isSafeInteger(tabPosition) || tabPosition < 0) throw new TypeError("getFocusedPane requires a tab position");
      const panes = record(manifest, "pane manifest").panes;
      if (panes === null || typeof panes !== "object") throw new TypeError("getFocusedPane requires a PaneManifest");
      return (panes[String(tabPosition)] ?? []).find((pane) => pane.is_focused === true && pane.is_plugin !== true) ?? null;
    },
    /** Run a command in an existing terminal pane, fire and forget: a bare number is a terminal id. */
    runCommandInPane: (pane, cmd, options = {}) => {
      const { context, keepScrollback = false } = record(options, "runCommandInPane options");
      if (typeof keepScrollback !== "boolean") throw new TypeError("keepScrollback must be a boolean");
      command("RunCommandInPane", { pane_id: { Terminal: terminalId(pane) }, command: record(cmd, "command"), context: stringMap(context, "context"), keep_scrollback: keepScrollback });
    },
    terminalPane: (id) => ({ Terminal: terminalId(id) }),
    pluginPane: (id) => {
      if (!Number.isSafeInteger(id) || id < 0 || id > 4294967295) throw new TypeError("plugin pane ID must fit u32");
      return { Plugin: id };
    },
    write: (text) => command("Write", bytes(text)),
    writeToPaneId: (text, pane) => command("WriteToPaneId", [bytes(text), pane]),
    readPluginLog: (name, lines = 50) => {
      if (!Number.isSafeInteger(lines) || lines < 0) throw new TypeError("readPluginLog lines must be a nonnegative integer");
      return command("ReadPluginLog", { name, lines });
    },
    openTerminal: (cwd) => {
      if (cwd !== undefined && cwd !== null && typeof cwd !== "string") {
        validate(cwd);
        cwd = record(cwd, "openTerminal cwd").cwd ?? cwd.path ?? null;
      }
      // FileToOpen defaults an absent path; a null one is refused.
      return command("OpenTerminal", cwd == null ? {} : { path: cwd });
    },
    closeSelf: () => command("CloseSelf"),
    setSelectable: (selectable) => {
      if (typeof selectable !== "boolean") throw new TypeError("setSelectable requires a boolean");
      return command("SetSelectable", selectable);
    },
    action,
    registerDefaultKeybindings,
    waitForPipe,
    getPluginIds: () => command("GetPluginIds"),
    log: logLine,
    waitForEvent,
    waitForPermissions: (permissions) => new Promise((resolve, reject) => enqueuePermissions(permissions, resolve, reject)),
  };
  Object.defineProperty(globalThis, "zellij", { value: api });

  function lifecycle(name, arg) {
    if (name === "load") {
      arg = { ...defaults, ...arg };
      if (arg.trace === "true") trace(true);
      else if (arg.trace === "false") trace(false);
      const namespace = arg.hocus_extension_name, path = arg.hocus_extension_path;
      hocusIdentity = typeof namespace === "string" && namespace && typeof path === "string" && path ? {namespace, path} : null;
    }
    if (tracing && name === "pipe") transcript(`pipe ${traceName(arg.name)}`);
    if (name === "pipe" && (completionPipe(arg) || rpcPipe(arg) || hocusPipe(arg) || keybindingPipe(arg))) return;
    const type = name === "update" ? (typeof arg === "string" ? arg : Object.keys(arg)[0]) : undefined;
    if (tracing && name === "update") transcript(`event ${traceName(type)}`);
    const payload = name === "update" && typeof arg !== "string" ? arg[type] : undefined;
    if (type === "PaneUpdate") {
      agentPanes = new Map(Object.values(payload.panes).flat().filter(p => !p.is_plugin).map(p => [p.id, {...p, agent:p.agent && {...p.agent}}]));
    }
    const listeners = [...(handlers.get(type) ?? [])];
    const waiters = name === "pipe" ? [...pipeWaiters] : [];
    // Starts are ordered by the executor. Promises are observed, never awaited here:
    // load may be waiting for the very permission event that update must deliver.
    if (typeof plugin[name] === "function") {
      track(plugin[name](arg), (result) => { if (result === true || name === "load") invalidate(); });
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
        rpcPrefix = arg.instanceId;
        metadata = arg.identity;
        metadata.swampTiming = arg.swampTiming;
        if (arg.workflowBridge) api.workflowRemotePanes = () => {
          if (client === null) throw new Error("host commands are unavailable during module initialization");
          return workflowRemotePanes();
        };
        Object.freeze(api);
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
