//! Explicit epoch and elapsed time units, retaining their scalar wire representations.
use serde::{Deserialize, Serialize};

macro_rules! time_unit {
    ($(#[$meta:meta])* $name:ident($scalar:ty) $(, $derive:ident)*) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, PartialOrd, Serialize, Deserialize $(, $derive)*)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name($scalar);
        impl $name {
            pub const fn new(value: $scalar) -> Self { Self(value) }
            pub const fn get(self) -> $scalar { self.0 }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
        }
    };
}

time_unit!(
    /// Absolute Unix milliseconds.
    /// ```compile_fail
    /// use yinz_types::units::{UnixMillis, ElapsedMillis};
    /// let _: UnixMillis = ElapsedMillis::new(1);
    /// ```
    UnixMillis(u64), Eq, Ord, Hash
);
time_unit!(
    /// Signed Unix milliseconds, including instants before the Unix epoch.
    SignedUnixMillis(i64), Eq, Ord, Hash
);
time_unit!(
    /// Absolute Unix seconds.
    UnixSeconds(u64), Eq, Ord, Hash
);
time_unit!(
    /// An elapsed number of milliseconds, including zero-disable settings.
    ElapsedMillis(u64), Eq, Ord, Hash
);
time_unit!(
    /// An elapsed number of seconds, including zero-disable settings.
    /// ```compile_fail
    /// use yinz_types::units::{ElapsedSeconds, ElapsedMillis};
    /// let _: ElapsedSeconds = ElapsedMillis::new(1);
    /// ```
    ElapsedSeconds(u64), Eq, Ord, Hash
);
time_unit!(
    /// Fractional seconds. Validation stays with the timer or protocol adapter.
    /// ```compile_fail
    /// use yinz_types::units::{ElapsedSeconds, FractionalSeconds};
    /// let _: FractionalSeconds = ElapsedSeconds::new(1);
    /// ```
    FractionalSeconds(f64)
);

impl ElapsedSeconds {
    pub fn checked_millis(self) -> Option<ElapsedMillis> {
        self.get().checked_mul(1000).map(ElapsedMillis::new)
    }
    /// Old KDL scalar options encode the signed integer's bits as an unsigned timeout.
    pub const fn from_legacy_signed_bits(value: i64) -> Self {
        Self::new(u64::from_ne_bytes(value.to_ne_bytes()))
    }
    pub const fn legacy_signed_bits(self) -> i64 {
        i64::from_ne_bytes(self.get().to_ne_bytes())
    }
}
impl UnixSeconds {
    pub fn checked_millis(self) -> Option<UnixMillis> {
        self.get().checked_mul(1000).map(UnixMillis::new)
    }
}
impl TryFrom<SignedUnixMillis> for UnixMillis {
    type Error = std::num::TryFromIntError;
    fn try_from(value: SignedUnixMillis) -> Result<Self, Self::Error> {
        u64::try_from(value.get()).map(Self::new)
    }
}
impl TryFrom<UnixMillis> for SignedUnixMillis {
    type Error = std::num::TryFromIntError;
    fn try_from(value: UnixMillis) -> Result<Self, Self::Error> {
        i64::try_from(value.get()).map(Self::new)
    }
}

impl TryFrom<u128> for UnixMillis {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u128) -> Result<Self, Self::Error> {
        u64::try_from(value).map(Self::new)
    }
}

impl TryFrom<u128> for SignedUnixMillis {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u128) -> Result<Self, Self::Error> {
        i64::try_from(value).map(Self::new)
    }
}

impl TryFrom<u128> for ElapsedMillis {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u128) -> Result<Self, Self::Error> {
        u64::try_from(value).map(Self::new)
    }
}

#[cfg(test)]
#[path = "unit/units_tests.rs"]
mod tests;
