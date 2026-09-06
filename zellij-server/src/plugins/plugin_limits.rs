//! What a plugin is allowed to allocate, and what we say when it isn't.
//!
//! Wasm linear memory only ever grows — the spec has no `memory.shrink`, and
//! wasmi's `Memory` offers no way to give pages back — so the number below is a
//! high-water mark rather than a live-set budget. A plugin that frees everything
//! it allocated is still, as far as this limiter can tell, as large as its
//! largest moment ever was. That makes a tight ceiling a poor deal: it buys
//! nothing back at runtime and eventually kills a plugin whose *peak* crossed
//! it, however little it holds the rest of the time.
//!
//! This is a hand-written [`ResourceLimiter`] rather than wasmi's own
//! `StoreLimits` for one reason: `StoreLimits` denies silently. All that reaches
//! the log is the bare trap string — "growth operation limited" — with no plugin
//! id, no sizes, and no way to tell a memory denial from a table one. The
//! numbers are the same shape as before; the difference is that a refusal now
//! says who asked for how much.

// `LimiterError` is what `ResourceLimiter` returns, but `wasmi` keeps its
// `core` re-export module private, so it has to be named through `wasmi_core`
// itself. Same crate, same type — `wasmi::ResourceLimiter` is a re-export of
// `wasmi_core::ResourceLimiter`.
use wasmi::ResourceLimiter;
use wasmi_core::LimiterError;

use crate::plugins::PluginId;

/// A plugin's linear memory ceiling.
///
/// Generous on purpose. Because memory never shrinks, this bounds the peak of
/// everything a plugin ever did at once, not what it holds now — and a plugin
/// that keeps a conversation reaches its peak while serializing one, not while
/// sitting on one. Reaching this is a runaway, not a busy session.
///
/// It is a ceiling and not a reservation: wasmi grows its backing buffer on
/// demand, so a plugin that uses three megabytes costs three megabytes.
pub const MAX_PLUGIN_MEMORY: usize = 512 * 1024 * 1024;

/// One instance per store. Workers get a store of their own.
const MAX_PLUGIN_INSTANCES: usize = 1;

/// Linear memories per plugin. A Rust wasm module declares one.
const MAX_PLUGIN_MEMORIES: usize = 4;

/// Tables per plugin. Not a cap on their elements, which stays unbounded.
const MAX_PLUGIN_TABLES: usize = 16;

pub struct PluginResourceLimits {
    plugin_id: PluginId,
    memory_size: usize,
}

impl PluginResourceLimits {
    pub fn new(plugin_id: PluginId) -> Self {
        Self {
            plugin_id,
            memory_size: MAX_PLUGIN_MEMORY,
        }
    }
}

impl ResourceLimiter for PluginResourceLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, LimiterError> {
        if desired > self.memory_size {
            log::error!(
                "Plugin {} asked to grow its memory to {}, past its {} limit (currently {})",
                self.plugin_id,
                mib(desired),
                mib(self.memory_size),
                mib(current),
            );
            return Err(LimiterError::ResourceLimiterDeniedAllocation);
        }
        if let Some(maximum) = maximum {
            if desired > maximum {
                log::error!(
                    "Plugin {} asked to grow its memory to {}, past the {} its module declares",
                    self.plugin_id,
                    mib(desired),
                    mib(maximum),
                );
                return Err(LimiterError::ResourceLimiterDeniedAllocation);
            }
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> Result<bool, LimiterError> {
        if let Some(maximum) = maximum {
            if desired > maximum {
                log::error!(
                    "Plugin {} asked to grow a table to {} elements, past the {} its module \
                     declares (currently {})",
                    self.plugin_id,
                    desired,
                    maximum,
                    current,
                );
                return Err(LimiterError::ResourceLimiterDeniedAllocation);
            }
        }
        Ok(true)
    }

    fn instances(&self) -> usize {
        MAX_PLUGIN_INSTANCES
    }

    fn tables(&self) -> usize {
        MAX_PLUGIN_TABLES
    }

    fn memories(&self) -> usize {
        MAX_PLUGIN_MEMORIES
    }
}

/// Sizes as they read in a log line. Wasm grows a page at a time, so every one
/// of these is a whole number of 64KiB pages and rounding down is honest.
fn mib(bytes: usize) -> String {
    format!("{}MiB", bytes / (1024 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> PluginResourceLimits {
        PluginResourceLimits::new(PluginId::new(1))
    }

    /// `LimiterError` carries no `PartialEq`, and the distinction that matters
    /// is not which variant came back but that one did: an `Err` traps, an
    /// `Ok(false)` doesn't.
    fn denied(result: Result<bool, LimiterError>) -> bool {
        matches!(result, Err(LimiterError::ResourceLimiterDeniedAllocation))
    }

    #[test]
    fn a_plugin_may_grow_up_to_the_ceiling() {
        assert_eq!(
            limits().memory_growing(0, MAX_PLUGIN_MEMORY, None).ok(),
            Some(true),
            "the ceiling itself is allowed, not just everything under it"
        );
    }

    /// Not `Ok(false)`: that hands the guest a `-1` from `memory.grow`, which
    /// Rust's allocator turns into an abort somewhere else entirely. Denying
    /// with an error traps at the instruction that asked, which is the only
    /// place a backtrace is worth anything.
    #[test]
    fn growing_past_the_ceiling_traps_rather_than_failing_quietly() {
        assert!(denied(limits().memory_growing(
            MAX_PLUGIN_MEMORY,
            MAX_PLUGIN_MEMORY + 1,
            None
        )));
    }

    /// A module may declare a maximum below ours, and that maximum wins.
    #[test]
    fn a_modules_own_maximum_still_binds_under_the_ceiling() {
        let under_our_ceiling = 1024 * 1024;
        assert_eq!(
            limits()
                .memory_growing(0, under_our_ceiling, Some(under_our_ceiling))
                .ok(),
            Some(true)
        );
        assert!(denied(limits().memory_growing(
            0,
            under_our_ceiling + 1,
            Some(under_our_ceiling)
        )));
    }

    /// Tables are capped in number, not in elements — as they were before.
    #[test]
    fn tables_grow_unbounded_unless_the_module_says_otherwise() {
        assert_eq!(limits().table_growing(0, usize::MAX, None).ok(), Some(true));
        assert!(denied(limits().table_growing(0, 17, Some(16))));
    }
}
