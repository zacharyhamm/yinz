//! Small helpers for composing SQL by hand.
//!
//! Everything here quotes; nothing here validates. A name reaches these helpers only after
//! [`crate::plugin_data::validate_name`] has accepted it, which is what keeps a plugin from
//! writing SQL — see the module doc on [`super`].

use crate::plugin_data::DataError;

pub(super) fn storage_err(context: &str, e: impl std::fmt::Display) -> DataError {
    DataError::Storage(format!("{}: {}", context, e))
}
