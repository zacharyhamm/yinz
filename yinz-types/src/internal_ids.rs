//! Canonical identities shared across crate boundaries.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A terminal identity.
///
/// ```compile_fail
/// use yinz_types::internal_ids::{TerminalId, PluginId};
/// let _: TerminalId = PluginId::new(0);
/// ```
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct TerminalId(u32);

impl TerminalId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl TryFrom<i64> for TerminalId {
    type Error = std::num::TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}

impl fmt::Display for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A plugin identity.
///
/// ```compile_fail
/// use yinz_types::internal_ids::{PluginId, ClientId};
/// let _: PluginId = ClientId::new(0);
/// ```
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct PluginId(u32);

impl PluginId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A client identity.
///
/// ```compile_fail
/// use yinz_types::internal_ids::{ClientId, TerminalId};
/// let _: ClientId = TerminalId::new(0);
/// ```
///
/// ```compile_fail
/// use yinz_types::internal_ids::ClientId;
/// fn for_client(_: ClientId) {}
/// for_client(1_u32);
/// ```
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ClientId(u32);

impl From<u32> for ClientId {
    fn from(value: u32) -> Self {
        Self::new(value)
    }
}

impl TryFrom<i64> for ClientId {
    type Error = std::num::TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}

impl ClientId {
    /// No client: a change or request that no connection owns. Connections are numbered from
    /// 1 and a session never gives a departed connection's id to another, so this is never a
    /// client's id.
    pub const NONE: ClientId = ClientId::new(0);

    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for TerminalId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<u32>().map(Self::new)
    }
}

impl std::str::FromStr for PluginId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<u32>().map(Self::new)
    }
}

impl std::str::FromStr for ClientId {
    type Err = std::num::ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<u32>().map(Self::new)
    }
}

impl TryFrom<u64> for TerminalId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<usize> for TerminalId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<i64> for PluginId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<u64> for PluginId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<usize> for PluginId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<u64> for ClientId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<usize> for ClientId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}

macro_rules! numeric_identity {
    ($(#[$doc:meta])* $name:ident($raw:ty)) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name($raw);
        impl $name {
            pub const fn new(value: $raw) -> Self { Self(value) }
            pub const fn get(self) -> $raw { self.0 }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) }
        }
        impl std::str::FromStr for $name {
            type Err = std::num::ParseIntError;
            fn from_str(value: &str) -> Result<Self, Self::Err> { value.parse().map(Self::new) }
        }
    };
}

macro_rules! unsigned_identity {
    ($(#[$doc:meta])* $name:ident(u64)) => {
        numeric_identity!($(#[$doc])* $name(u64));
        impl TryFrom<i64> for $name {
            type Error = std::num::TryFromIntError;
            fn try_from(value: i64) -> Result<Self, Self::Error> {
                u64::try_from(value).map(Self::new)
            }
        }
    };
    ($(#[$doc:meta])* $name:ident($raw:ty)) => {
        numeric_identity!($(#[$doc])* $name($raw));
        impl TryFrom<u64> for $name {
            type Error = std::num::TryFromIntError;
            fn try_from(value: u64) -> Result<Self, Self::Error> { <$raw>::try_from(value).map(Self::new) }
        }
        impl TryFrom<i64> for $name {
            type Error = std::num::TryFromIntError;
            fn try_from(value: i64) -> Result<Self, Self::Error> { <$raw>::try_from(value).map(Self::new) }
        }
    };
}

unsigned_identity!(
    /// A stable tab identity. Moving a tab does not change this value.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{TabId, TabPosition};
    /// let _: TabId = TabPosition::new(0);
    /// ```
    TabId(usize)
);
unsigned_identity!(
    /// A zero-based tab position in the current display order.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{TabPosition, TabNumber};
    /// let _: TabPosition = TabNumber::new(1);
    /// ```
    TabPosition(usize)
);
unsigned_identity!(
    /// A one-based tab number used by CLI actions and legacy plugin commands.
    /// Zero retains its legacy meaning: the first tab, like one.
    TabNumber(u32)
);

unsigned_identity!(
    /// A plugin data watch identity, scoped to the server process.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{DataWatchId, NotificationId};
    /// let _: DataWatchId = NotificationId::new(0);
    /// ```
    DataWatchId(u64)
);
unsigned_identity!(
    /// A notification identity, including zero for the first notification.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{NotificationId, ContextMenuRequestId};
    /// let _: NotificationId = ContextMenuRequestId::new(0);
    /// ```
    NotificationId(u64)
);
unsigned_identity!(
    /// A context menu request identity, echoed by its responding plugin.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{ContextMenuRequestId, ContextMenuItemId};
    /// let _: ContextMenuRequestId = ContextMenuItemId::new(0);
    /// ```
    ContextMenuRequestId(u64)
);
unsigned_identity!(
    /// A configured context menu item identity, retained across a plugin hook.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{ContextMenuItemId, ContextMenuRequestId};
    /// let _: ContextMenuItemId = ContextMenuRequestId::new(0);
    /// ```
    ContextMenuItemId(u64)
);

numeric_identity!(
    /// A plugin data row identity. Its signed representation matches SQL and the plugin protocol.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{DataRowId, DataRevision};
    /// let _: DataRowId = DataRevision::new(1);
    /// ```
    DataRowId(i64)
);
impl TryFrom<u64> for DataRowId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        i64::try_from(value).map(Self::new)
    }
}
numeric_identity!(
    /// The revision read by a plugin data writer. Zero retains the absent-row sentinel.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{DataRevision, DataRowId};
    /// let _: DataRevision = DataRowId::new(1);
    /// ```
    DataRevision(i64)
);
impl TryFrom<u64> for DataRevision {
    type Error = std::num::TryFromIntError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        i64::try_from(value).map(Self::new)
    }
}

macro_rules! string_identity {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            pub fn into_string(self) -> String {
                self.0
            }
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }
        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }
        impl std::str::FromStr for $name {
            type Err = std::convert::Infallible;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Ok(Self::new(value))
            }
        }
        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

string_identity!(
    /// The installation that owns shared data, distinct from its display hostname.
    /// An empty identity retains the meaning of a legacy, unclaimed row.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::HostId;
    /// let display_hostname = String::from("laptop");
    /// let _: HostId = display_hostname;
    /// ```
    HostId
);

string_identity!(
    /// The stable identity of a pane supervisor, retained when a pane moves between sessions.
    /// Empty in legacy remote-pane handoffs, whose broker token supplies the address instead.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{SupervisorId, HostId};
    /// let _: SupervisorId = HostId::new("host-a");
    /// ```
    SupervisorId
);

unsigned_identity!(
    /// An operating-system process identity, distinct from a terminal pane identity.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{ProcessId, TerminalId};
    /// let _: ProcessId = TerminalId::new(1);
    /// ```
    ProcessId(u32)
);
impl ProcessId {
    /// The legacy plugin int32 fields carry the unsigned PID's bits.
    pub const fn from_legacy_signed_bits(value: i32) -> Self {
        Self::new(u32::from_ne_bytes(value.to_ne_bytes()))
    }
    /// Encode a legacy plugin field. Use checked `i32::try_from` for kernel calls.
    pub const fn legacy_signed_bits(self) -> i32 {
        i32::from_ne_bytes(self.get().to_ne_bytes())
    }
}

pub(crate) mod process_id_legacy_signed {
    use super::ProcessId;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(id: &ProcessId, serializer: S) -> Result<S::Ok, S::Error> {
        id.legacy_signed_bits().serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ProcessId, D::Error> {
        i32::deserialize(deserializer).map(ProcessId::from_legacy_signed_bits)
    }
}

impl TryFrom<i32> for ProcessId {
    type Error = std::num::TryFromIntError;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}
impl TryFrom<ProcessId> for i32 {
    type Error = std::num::TryFromIntError;
    fn try_from(value: ProcessId) -> Result<Self, Self::Error> {
        i32::try_from(value.get())
    }
}

impl TryFrom<usize> for TabNumber {
    type Error = std::num::TryFromIntError;
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u32::try_from(value).map(Self::new)
    }
}

impl TabNumber {
    pub fn position(self) -> TabPosition {
        TabPosition::new(self.get().saturating_sub(1) as usize)
    }
}
impl TryFrom<TabPosition> for TabNumber {
    type Error = &'static str;
    fn try_from(position: TabPosition) -> Result<Self, Self::Error> {
        position
            .get()
            .checked_add(1)
            .and_then(|value| u32::try_from(value).ok())
            .map(Self::new)
            .ok_or("Tab position is outside the one-based tab number range")
    }
}

#[cfg(test)]
#[path = "unit/internal_ids_tests.rs"]
mod tests;

string_identity!(
    /// A Yinz session identity, distinct from host and agent-provider identities.
    /// Validation remains at the session creation and rename boundary.
    ///
    /// ```compile_fail
    /// use yinz_types::internal_ids::{SessionName, HostId};
    /// let _: SessionName = HostId::new("main");
    /// ```
    SessionName
);

impl AsRef<std::path::Path> for SessionName {
    fn as_ref(&self) -> &std::path::Path {
        std::path::Path::new(self.as_str())
    }
}
