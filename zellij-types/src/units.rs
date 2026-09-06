//! Explicit units for persisted recording times.
use serde::{Deserialize, Serialize};

/// An absolute Unix timestamp in signed microseconds.
///
/// ```compile_fail
/// use zellij_types::units::{UnixMicros, ElapsedMicros};
/// let _: UnixMicros = ElapsedMicros::new(0);
/// ```
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct UnixMicros(i64);

impl UnixMicros {
    pub const fn new(value: i64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Elapsed time in signed microseconds, retaining negative legacy values.
///
/// ```compile_fail
/// use zellij_types::units::{UnixMicros, ElapsedMicros};
/// let _: UnixMicros = ElapsedMicros::new(0);
/// ```
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ElapsedMicros(i64);

impl ElapsedMicros {
    pub const fn new(value: i64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> i64 {
        self.0
    }
}
