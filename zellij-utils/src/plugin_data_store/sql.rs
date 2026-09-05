//! Small helpers for composing SQL by hand, and for saying what failed when a statement does.
//!
//! Everything here quotes; nothing here validates. A name reaches these helpers only after
//! [`crate::plugin_data::validate_name`] has accepted it, which is what keeps a plugin from
//! writing SQL — see the module doc on [`super`].

use crate::plugin_data::DataError;

pub(super) fn storage_err(context: &str, e: impl std::fmt::Display) -> DataError {
    DataError::Storage(format!("{}: {}", context, e))
}

/// An error type a database driver produces. Implemented by each backend for its driver's error,
/// so that [`SqlContext`] is available on driver results without a blanket impl that would also
/// match `Result<_, DataError>`.
pub(super) trait StorageError: std::fmt::Display {
    fn into_data_error(self, context: &str) -> DataError
    where
        Self: Sized,
    {
        storage_err(context, self)
    }
}

/// Attach context to a failed driver call.
///
/// The alternative is `.map_err(|e| storage_err("…", e))` at every call site, which this module
/// had thirty-six of.
pub(super) trait SqlContext<T> {
    fn ctx(self, context: &str) -> Result<T, DataError>;

    /// For a context string that has to be built, so the `format!` only runs on the error path.
    fn with_ctx(self, context: impl FnOnce() -> String) -> Result<T, DataError>;
}

impl<T, E: StorageError> SqlContext<T> for Result<T, E> {
    fn ctx(self, context: &str) -> Result<T, DataError> {
        self.map_err(|e| e.into_data_error(context))
    }

    fn with_ctx(self, context: impl FnOnce() -> String) -> Result<T, DataError> {
        self.map_err(|e| e.into_data_error(&context()))
    }
}

/// Attach context to a storage failure that has already been mapped to a [`DataError`].
///
/// Only `Storage` errors are prefixed. Every other variant carries a message written for the
/// plugin author about their own request, and a "failed to run the fetch" in front of "`x` is
/// not a declared attribute" would say less, not more.
pub(super) trait StoreContext<T> {
    fn context(self, context: &str) -> Result<T, DataError>;

    fn with_context(self, context: impl FnOnce() -> String) -> Result<T, DataError>;
}

impl<T> StoreContext<T> for Result<T, DataError> {
    fn context(self, context: &str) -> Result<T, DataError> {
        self.map_err(|e| match e {
            DataError::Storage(message) => DataError::Storage(format!("{}: {}", context, message)),
            other => other,
        })
    }

    fn with_context(self, context: impl FnOnce() -> String) -> Result<T, DataError> {
        self.map_err(|e| match e {
            DataError::Storage(message) => {
                DataError::Storage(format!("{}: {}", context(), message))
            },
            other => other,
        })
    }
}

/// One identifier, quoted: `name` becomes `"name"`.
///
/// Both engines quote identifiers with double quotes and double an embedded one. A name that
/// reached here has already been through `validate_name` and cannot contain a quote, but doubling
/// it costs nothing and means this function is correct on its own terms rather than only in
/// context.
pub(super) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A column qualified by its table: `"table"."column"`.
pub(super) fn qualify(table: &str, column: &str) -> String {
    format!("{}.{}", quote_ident(table), quote_ident(column))
}

/// A comma-separated list of quoted identifiers, each behind an optional prefix.
///
/// The prefix is what SQLite's FTS triggers need: the same column list rendered once bare, once
/// as `new."col"` and once as `old."col"`.
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
