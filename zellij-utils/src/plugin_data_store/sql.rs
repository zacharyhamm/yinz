//! Small helpers for composing SQL by hand.
//!
//! Everything here quotes; nothing here validates. A name reaches these helpers only after
//! [`crate::plugin_data::validate_name`] has accepted it, which is what keeps a plugin from
//! writing SQL — see the module doc on [`super`].

use crate::plugin_data::DataError;

pub(super) fn storage_err(context: &str, e: impl std::fmt::Display) -> DataError {
    DataError::Storage(format!("{}: {}", context, e))
}

/// Attach context to a failed `rusqlite` call.
///
/// The alternative is `.map_err(|e| storage_err("…", e))` at every call site, which this module
/// had thirty-six of.
pub(super) trait SqlContext<T> {
    fn ctx(self, context: &str) -> Result<T, DataError>;

    /// For a context string that has to be built, so the `format!` only runs on the error path.
    fn with_ctx(self, context: impl FnOnce() -> String) -> Result<T, DataError>;
}

impl<T> SqlContext<T> for Result<T, rusqlite::Error> {
    fn ctx(self, context: &str) -> Result<T, DataError> {
        self.map_err(|e| storage_err(context, e))
    }

    fn with_ctx(self, context: impl FnOnce() -> String) -> Result<T, DataError> {
        self.map_err(|e| storage_err(&context(), e))
    }
}

/// One identifier, quoted: `name` becomes `"name"`.
///
/// SQLite's quoting rule is to double an embedded quote. A name that reached here has already been
/// through `validate_name` and cannot contain one, but doubling it costs nothing and means this
/// function is correct on its own terms rather than only in context.
pub(super) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A column qualified by its table: `"table"."column"`.
pub(super) fn qualify(table: &str, column: &str) -> String {
    format!("{}.{}", quote_ident(table), quote_ident(column))
}

/// A comma-separated list of quoted identifiers, each behind an optional prefix.
///
/// The prefix is what the FTS triggers need: the same column list rendered once bare, once as
/// `new."col"` and once as `old."col"`.
pub(super) fn join_idents<I, S>(prefix: &str, names: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    names
        .into_iter()
        .map(|name| format!("{}{}", prefix, quote_ident(name.as_ref())))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `?, ?, ?` — one placeholder per bound parameter.
pub(super) fn placeholders(count: usize) -> String {
    vec!["?"; count].join(", ")
}
