//! What the store knows about one entity, and the names it derives from that.
//!
//! Every name an entity's storage goes by — its table, its full-text index, its per-attribute
//! indexes — is derived from the registry row id here, so the `e_<row_id>` scheme is stated in one
//! place and no plugin-supplied string reaches a table name.

use yinz_types::plugin_data::{AttrDef, AttrKind, DataError, EntityDef};

/// The store's own columns, present on every entity table.
///
/// The order matches [`super::dialect::Dialect::store_columns`], which supplies the definitions.
pub(super) const STORE_COLUMNS: &[&str] = &["id", "uid", "created_at", "updated_at", "rev"];

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

    /// SQLite's external-content FTS5 table.
    pub(super) fn fts_table(&self) -> String {
        format!("e_{}_fts", self.row_id)
    }

    pub(super) fn attr_index(&self, attr: &str) -> String {
        format!("e_{}_a_{}", self.row_id, attr)
    }

    pub(super) fn unique_index(&self, attr: &str) -> String {
        format!("e_{}_u_{}", self.row_id, attr)
    }

    /// The indexes on `created_at` and `updated_at`, which every entity has because every list,
    /// sync and paginate query orders or filters on them.
    pub(super) fn timestamp_indexes(&self) -> [String; 2] {
        [
            format!("e_{}_created", self.row_id),
            format!("e_{}_updated", self.row_id),
        ]
    }

    /// The three triggers that keep SQLite's full-text index exact, after insert, delete and
    /// update.
    pub(super) fn fts_triggers(&self) -> [String; 3] {
        [
            format!("e_{}_fts_ai", self.row_id),
            format!("e_{}_fts_ad", self.row_id),
            format!("e_{}_fts_au", self.row_id),
        ]
    }

    pub(super) fn attr(&self, name: &str) -> Option<&AttrDef> {
        self.def.attr_named(name)
    }

    /// The kind of a column, or `None` for one of the store's own columns.
    pub(super) fn column_kind(&self, name: &str) -> Option<AttrKind> {
        self.attr(name).map(|a| a.kind)
    }

    /// The kind of any column, the store's own included: `uid` is the only text one, and `id`,
    /// `created_at` and `updated_at` are integers.
    pub(super) fn any_column_kind(&self, name: &str) -> AttrKind {
        self.column_kind(name).unwrap_or({
            if name == "uid" {
                AttrKind::Text
            } else {
                AttrKind::Int
            }
        })
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
