//! Per-isolate budgets. Exhaustion is permanent, even when JavaScript catches it.
use deno_core::{op2, v8, JsRuntime, OpState, RuntimeOptions};
use deno_error::JsErrorBox;
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Arc,
};
use zellij_runtime::errors::prelude::*;

const GIB: usize = 1024 * 1024 * 1024;
pub(super) const MAX_SOURCE_BYTES: usize = zellij_config::plugins::MAX_DENO_SOURCE_BYTES;
pub(super) const MAX_GRAPH_BYTES: usize = zellij_ipc::MAX_IPC_FRAME_LEN;

pub(super) struct Limits {
    identity: String,
    heap: usize,
    buffers: usize,
    live_buffers: AtomicUsize,
    failed: AtomicU8,
}

impl Limits {
    pub(super) fn new(identity: String) -> Arc<Self> {
        #[cfg(test)]
        if std::env::var_os("ZELLIJ_TEST_DENO_SMALL_BUDGET").is_some() {
            return Self::with_budgets(identity, 32 << 20, 16 << 20);
        }
        Self::with_budgets(identity, GIB, GIB)
    }

    fn with_budgets(identity: String, heap: usize, buffers: usize) -> Arc<Self> {
        Arc::new(Self {
            identity,
            heap,
            buffers,
            live_buffers: AtomicUsize::new(0),
            failed: AtomicU8::new(0),
        })
    }

    pub(super) fn exhausted(&self) -> bool {
        self.failed.load(Ordering::Acquire) != 0
    }

    fn fail(&self, resource: u8) {
        let _ = self
            .failed
            .compare_exchange(0, resource, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(super) fn check(&self) -> Result<()> {
        match self.failed.load(Ordering::Acquire) {
            0 => Ok(()),
            1 => bail!("Deno plugin {} exceeded V8 heap budget ({} bytes)", self.identity, self.heap),
            _ => bail!("Deno plugin {} exceeded ArrayBuffer backing-store budget ({} bytes), or allocation failed", self.identity, self.buffers),
        }
    }

    pub(super) fn runtime(self: &Arc<Self>, mut options: RuntimeOptions) -> Result<JsRuntime> {
        // SAFETY: the allocator owns one Arc, released only by its drop callback;
        // V8 retains the allocator until the last associated backing store is gone.
        let allocator = unsafe { v8::new_rust_allocator(Arc::into_raw(self.clone()), &ALLOCATOR) };
        options.create_params = Some(
            v8::CreateParams::default()
                .heap_limits(0, self.heap)
                .array_buffer_allocator(allocator),
        );
        let mut runtime = JsRuntime::try_new(options).context("create budgeted Deno runtime")?;
        let limits = self.clone();
        let interrupt = runtime.v8_isolate().thread_safe_handle();
        runtime.add_near_heap_limit_callback(move |current, _| {
            limits.fail(1);
            interrupt.terminate_execution();
            // Deno Core 0.405's tested termination pattern: temporary headroom
            // lets V8 unwind instead of reaching its fatal OOM handler.
            current.saturating_mul(2)
        });
        runtime.op_state().borrow_mut().put(self.clone());
        let initialized =
            runtime.execute_script("zellij:allocation-limits", include_str!("limits.js"));
        self.check()?;
        initialized.context("initialize Deno allocation limits")?;
        Ok(runtime)
    }

    fn reserve(&self, bytes: usize) -> bool {
        if self.exhausted()
            || self
                .live_buffers
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                    live.checked_add(bytes)
                        .filter(|total| *total <= self.buffers)
                })
                .is_err()
        {
            self.fail(2);
            false
        } else {
            true
        }
    }
}

// No allocator callback may enter V8. The runtime watcher interrupts caught
// allocation failures; invocation boundaries also check the permanent flag.
unsafe extern "C" fn allocate(limits: &Limits, bytes: usize) -> *mut c_void {
    allocate_bytes(limits, bytes, true)
}
unsafe extern "C" fn allocate_uninitialized(limits: &Limits, bytes: usize) -> *mut c_void {
    allocate_bytes(limits, bytes, false)
}
fn allocate_bytes(limits: &Limits, bytes: usize, zeroed: bool) -> *mut c_void {
    if !limits.reserve(bytes) {
        return std::ptr::null_mut();
    }
    // malloc/calloc report failure, unlike Rust's infallible allocation APIs.
    // Allocate one byte for a zero-length request so null always means failure.
    let data = unsafe {
        if zeroed {
            libc::calloc(1, bytes.max(1))
        } else {
            libc::malloc(bytes.max(1))
        }
    };
    if data.is_null() {
        limits.live_buffers.fetch_sub(bytes, Ordering::AcqRel);
        limits.fail(2);
    }
    data
}
unsafe extern "C" fn free(limits: &Limits, data: *mut c_void, bytes: usize) {
    if !data.is_null() {
        unsafe { libc::free(data) };
        limits.live_buffers.fetch_sub(bytes, Ordering::AcqRel);
    }
}
unsafe extern "C" fn drop_allocator(limits: *const Limits) {
    drop(unsafe { Arc::from_raw(limits) });
}
static ALLOCATOR: v8::RustAllocatorVtable<Limits> = v8::RustAllocatorVtable {
    allocate,
    allocate_uninitialized,
    free,
    drop: drop_allocator,
};

/// Bytes charged to an isolate's backing-store budget, on their way to becoming a
/// `Uint8Array` it owns.
///
/// `serde_v8`'s buffer types build their stores from Rust allocations that the isolate's
/// allocator never sees, so anything the host hands the guest as bytes - an encoded string,
/// a streamed output chunk, a byte reply - goes through here instead, and is freed by the
/// same counted deleter V8 uses for the guest's own buffers.
pub(super) struct CountedAllocation {
    limits: Arc<Limits>,
    data: *mut c_void,
    bytes: usize,
}

impl CountedAllocation {
    pub(super) fn new(limits: &Arc<Limits>, bytes: usize) -> Result<Self> {
        let data = allocate_bytes(limits, bytes, true);
        if data.is_null() {
            limits.check()?;
            bail!("host byte buffer exceeded ArrayBuffer backing-store budget");
        }
        Ok(Self {
            limits: limits.clone(),
            data,
            bytes,
        })
    }

    pub(super) fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: this allocation belongs exclusively to us until V8 takes ownership.
        unsafe { std::slice::from_raw_parts_mut(self.data.cast::<u8>(), self.bytes) }
    }

    /// Hand the allocation to V8. From here on its release runs the counted deleter.
    pub(super) fn into_array<'s>(
        mut self,
        scope: &mut v8::PinScope<'s, '_>,
    ) -> Result<v8::Local<'s, v8::Uint8Array>> {
        unsafe extern "C" fn release(data: *mut c_void, bytes: usize, context: *mut c_void) {
            let limits = unsafe { Arc::from_raw(context.cast::<Limits>()) };
            unsafe { free(&limits, data, bytes) };
        }
        let data = std::mem::replace(&mut self.data, std::ptr::null_mut());
        // SAFETY: the store owns this allocation and one Arc until release runs.
        let backing = unsafe {
            v8::ArrayBuffer::new_backing_store_from_ptr(
                data,
                self.bytes,
                release,
                Arc::into_raw(self.limits.clone()) as *mut c_void,
            )
        }
        .make_shared();
        let buffer = v8::ArrayBuffer::with_backing_store(scope, &backing);
        v8::Uint8Array::new(scope, buffer, 0, self.bytes).context("cannot create byte array")
    }
}

impl Drop for CountedAllocation {
    fn drop(&mut self) {
        // Only reached when the allocation was never handed to V8: `free` ignores null.
        unsafe { free(&self.limits, self.data, self.bytes) };
    }
}

/// A `Uint8Array` holding a copy of `data`, charged to the isolate's budget.
pub(super) fn counted_bytes<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    limits: &Arc<Limits>,
    data: &[u8],
) -> Result<v8::Local<'s, v8::Uint8Array>> {
    let mut allocation = CountedAllocation::new(limits, data.len())?;
    allocation.as_mut_slice().copy_from_slice(data);
    allocation.into_array(scope)
}

// Core's encoder creates a Rust-owned backing store outside the isolate's
// allocator. Use V8's UTF-8 encoder with the same quota and a counted deleter.
#[op2]
pub(super) fn op_zellij_encode<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    state: &OpState,
    text: v8::Local<'s, v8::String>,
) -> std::result::Result<v8::Local<'s, v8::Uint8Array>, JsErrorBox> {
    let limits = state.borrow::<Arc<Limits>>();
    let bytes = text.utf8_length(scope);
    let mut allocation = CountedAllocation::new(limits, bytes).map_err(|_| {
        JsErrorBox::generic("UTF-8 encoding exceeded ArrayBuffer backing-store budget")
    })?;
    text.write_utf8_v2(
        scope,
        allocation.as_mut_slice(),
        v8::WriteFlags::kReplaceInvalidUtf8,
        None,
    );
    allocation
        .into_array(scope)
        .map_err(|error| JsErrorBox::generic(format!("{error:#}")))
}

pub(super) fn check_source(path: &std::path::Path, source: &str) -> Result<()> {
    if source.len() > MAX_SOURCE_BYTES {
        bail!(
            "Deno plugin {} exceeds 8 MiB source-file budget",
            path.display()
        );
    }
    Ok(())
}

pub(super) fn read_source(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    let mut source = String::new();
    std::fs::File::open(path)
        .with_context(|| format!("open Deno module {}", path.display()))?
        .take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_string(&mut source)
        .with_context(|| format!("read Deno module {}", path.display()))?;
    check_source(path, &source)?;
    Ok(source)
}

#[cfg(test)]
#[path = "unit/limits.rs"]
mod tests;

/// Conservative native-state reservation, released with its CppGC owner.
/// Mutation inputs remain charged until collection, covering retained capacities and caches.
/// ponytail: long-lived widgets retain charges; measure capacities if they exhaust the quota.
pub(super) struct NativeCharge {
    limits: Arc<Limits>,
    bytes: usize,
}
impl NativeCharge {
    pub(super) fn new(limits: Arc<Limits>) -> Self {
        Self { limits, bytes: 0 }
    }
    pub(super) fn grow(&mut self, bytes: usize) -> Result<()> {
        if !self.limits.reserve(bytes) {
            self.limits.check()?;
        }
        self.bytes += bytes;
        Ok(())
    }
}
impl Drop for NativeCharge {
    fn drop(&mut self) {
        self.limits
            .live_buffers
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
