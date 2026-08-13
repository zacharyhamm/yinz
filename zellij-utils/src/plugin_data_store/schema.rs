//! What the store knows about one entity, and the names it derives from that.
//!
//! Every name an entity's storage goes by — its table, its FTS table, its per-attribute indexes —
//! is derived from the registry row id here, so the `e_<row_id>` scheme is stated in one place and
//! no plugin-supplied string reaches a table name.

use crate::plugin_data::{AttrDef, AttrKind, DataError, EntityDef};

/// The store's own columns, present on every entity table.
pub(super) const STORE_COLUMNS: &[&str] = &["id", "uid", "created_at", "updated_at"];

/// A stored entity, as read back from the registry.
#[derive(Debug, Clone)]
pub(super) struct EntitySchema {
    pub(super) row_id: i64,
    pub(super) def: EntityDef,
    pub(super) owner_plugin: String,
}

impl EntitySchema {
    pub(super) fn table(&self) -> String {
        format!("e_{}", self.row_id)
    }

    pub(super) fn fts_table(&self) -> String {
        format!("e_{}_fts", self.row_id)
    }

    pub(super) fn attr_index(&self, attr: &str) -> String {
        format!("e_{}_a_{}", self.row_id, attr)
    }

    pub(super) fn unique_index(&self, attr: &str) -> String {
        format!("e_{}_u_{}", self.row_id, attr)
    }

    pub(super) fn attr(&self, name: &str) -> Option<&AttrDef> {
        self.def.attr_named(name)
    }

    /// The kind of a column, or `None` for one of the store's own columns.
    pub(super) fn column_kind(&self, name: &str) -> Option<AttrKind> {
        self.attr(name).map(|a| a.kind)
    }

    /// Reject a column name that is neither a declared attribute nor one of the store's own.
    pub(super) fn require_column(&self, name: &str) -> Result<(), DataError> {
        if STORE_COLUMNS.contains(&name) || self.attr(name).is_some() {
            return Ok(());
        }
        Err(DataError::InvalidQuery(format!(
            "`{}` is not a declared attribute of `{}`; it has {}",
            name,
            self.def.name,
            if self.def.attrs.is_empty() {
                "none".to_owned()
            } else {
                self.def
                    .attrs
                    .iter()
                    .map(|a| format!("`{}`", a.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )))
    }
}
