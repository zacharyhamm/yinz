# Local Steel patches

Source: `steel-core` 0.8.3 from crates.io, upstream `mattwparas/steel` commit
`11a57adcf42c22733463c59b1f44ec49eb7043bc`, directory `crates/steel-core`.
The original package SHA-256 is `1341b81ac9c27acc592a7f76eebeb4a4420d1099d7419f1b1ee9eb7fcca13a15`.
The packaged license and upstream source are retained.

Compiler and runtime fixes are applied:

- `src/compiler/compiler.rs` and `src/steel_vm/engine.rs`: expose a diagnostic-only
  source name for in-memory programs. Entry-point closures retain their filename
  and line after deferred execution, without canonicalizing a real path or
  granting the compiler a filesystem module origin. Existing path APIs retain
  their original behavior.

- `src/compiler/code_gen.rs`: specialize an integer immediate only when it fits
  the 24-bit instruction payload. Larger positive constants previously panicked
  in debug builds and were silently truncated in release builds.
- `src/steel_vm/const_evaluation.rs`: keep top-level definitions out of constant
  propagation. A later REPL `set!` must be visible to previously defined closures.
  Local lexical constant propagation remains enabled.

- `src/compiler/passes/begin.rs`: delay nonconstant internal definition initializers
  until their position in the body. When a body also defined a local function,
  these initializers previously ran before preceding effects such as `set!`.
  The existing delayed-initializer path preserves mutually recursive functions.

Two deprecated `std::usize::MAX` references in `src/compiler/passes/analysis.rs`
use the equivalent `usize::MAX` spelling to keep the current toolchain warning-free.
The unused `AsRefSteelVal` import in `src/steel_vm/vm/threads.rs` is removed.

Regression coverage is in `yinz-steel/src/unit/runtime.rs`. Remove each local
change when upgrading to an upstream release that passes those regressions.

- `primitives/control.rs`: preserve the contents of string arguments to `error`
  and `to-string`. Formatting a string through its Scheme representation escaped
  quotes, newlines, and backslashes again on every caught/rethrown error. Other
  values keep their existing printed representation and leading separators.

- `src/compiler/passes/analysis.rs`: include enclosing lambda arguments when
  propagating capture mutability. A closure created before a later `set!` otherwise
  received an internal box instead of the captured value.
- `src/values/closed.rs`: run the existing synchronous pointer-marking traversal on
  the interpreter thread. The upstream collector lazily spawned one marker thread
  per available CPU in every plugin process; ordinary larger inventories exhausted
  confined workers' address-space budget while starting that pool. Marking retains
  the same root and pointer traversal, then releases the temporary root queue.
  The confined-worker regression keeps a large mutable graph alive across repeated
  collections and verifies its values afterward.

- `src/rvals.rs`: expose a read-only `SteelByteVector::with_bytes` borrowing method.
  The host bridge can decode and transmit binary values without expanding each byte into
  a Scheme integer or JSON number. The callback cannot retain the borrowed slice.
