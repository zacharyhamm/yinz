// V8 reserves resizable/shared-growable and Wasm memory outside its ArrayBuffer
// allocator. Keep fixed buffers available, without exposing those quota bypasses.
(() => {
  const construct = Reflect.construct;
  for (const name of ["ArrayBuffer", "SharedArrayBuffer"]) {
    const Native = globalThis[name];
    if (!Native) continue;
    function FixedBuffer(length, options) {
      if (!new.target) throw new TypeError(`${name} requires new`);
      if (options !== undefined) throw new TypeError("resizable buffers are unavailable in plugins");
      return construct(Native, [length], new.target);
    }
    Object.defineProperty(FixedBuffer, "name", {value: name});
    // Copy static helpers without linking the constructor to Native.
    for (const key of Reflect.ownKeys(Native)) {
      if (!["length", "name", "prototype"].includes(key)) {
        Object.defineProperty(FixedBuffer, key, Object.getOwnPropertyDescriptor(Native, key));
      }
    }
    FixedBuffer.prototype = Native.prototype;
    Object.defineProperty(Native.prototype, "constructor", {value: FixedBuffer, writable: true, configurable: true});
    globalThis[name] = FixedBuffer;
  }
  delete globalThis.WebAssembly;
})();
