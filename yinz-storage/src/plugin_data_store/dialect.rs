//! SQL syntax supplied by each backend to the shared statement composers.

use yinz_types::plugin_data::{AttrKind, DataError, DataValue};

use super::schema::EntitySchema;

pub(super) trait Dialect: Send + Sync + 'static {
    /// The placeholder naming the `index`-th bound parameter, 1-based: `?3` or `$3`.
    ///
    /// Numbered so one parameter can be referenced twice in a statement.
    fn placeholder(&self, index: usize) -> String;

    /// Normalize a generated ASCII index name to the engine's identifier limit.
    fn index_name(&self, name: &str) -> String;

    /// The column type for a declared attribute, collation included where it matters.
    fn column_type(&self, kind: AttrKind) -> &'static str;

    /// The store's own columns, in the order of [`super::schema::STORE_COLUMNS`]:
    /// `id`, `uid`, `created_at`, `updated_at`, `rev`, each as a full column definition.
    fn store_columns(&self) -> [String; 5];

    /// The type and constraint of the `rev` column on its own, for the migration that adds it to
    /// a table created before it existed. Separate from [`Dialect::store_columns`] because an
    /// `ALTER TABLE ADD COLUMN` needs a default and a `CREATE TABLE` does not: an existing row
    /// has to land on some revision, and 1 is the one a row written today would have had.
    fn rev_column_added(&self) -> &'static str;

    /// `CREATE TABLE IF NOT EXISTS` for the entity registry.
    ///
    /// `IF NOT EXISTS` because this is migration 1 and a SQLite database predating the store's
    /// own version tracking already has the table — it is at version 0 by having no record, not
    /// by being empty, so step 1 runs against a registry that is already there.
    fn registry_ddl(&self) -> &'static str;

    /// `CREATE TABLE IF NOT EXISTS` for `store_meta`, which holds the schema version.
    fn store_meta_ddl(&self) -> &'static str;

    /// Render a default as a literal for a column definition.
    ///
    /// Defaults in DDL require literals rather than bound parameters.
    fn literal(&self, value: &DataValue) -> Result<String, DataError>;

    /// A case-insensitive pattern match of `column` against the parameter at `placeholder`.
    ///
    /// SQLite's `LIKE` folds ASCII case by default. The pattern has
    /// already been through [`super::predicate::escape_like`], which escapes with a backslash.
    fn like(&self, column: &str, placeholder: &str) -> String;

    /// DDL that builds the full-text index over `attrs` and backfills it from the rows already
    /// stored. Runs after [`Dialect::full_text_drop`], never on top of an existing index.
    fn full_text_create(&self, schema: &EntitySchema, attrs: &[&str]) -> String;

    /// DDL that removes the full-text index, whether or not one exists.
    ///
    /// Has to be able to run before an attribute column is dropped: SQLite refuses to drop a
    /// column their full-text machinery still names.
    fn full_text_drop(&self, schema: &EntitySchema) -> String;

    /// The pieces of a fetch that searches, with the search term bound at `placeholder`.
    fn search(&self, schema: &EntitySchema, placeholder: &str) -> SearchSql;

    /// Whether a search term the engine rejects as bad syntax should be retried as a literal
    /// phrase. FTS5 has a query language and errors on unbalanced quotes.
    fn retries_search_as_phrase(&self) -> bool;
}

/// The three pieces a search adds to a query: a join (possibly empty), a `WHERE` condition, and
/// an `ORDER BY` term that puts the best match first.
pub(super) struct SearchSql {
    pub(super) join: String,
    pub(super) condition: String,
    pub(super) rank: String,
}

/// The bound parameters of a statement being composed, and the dialect that names them.
///
/// Binding returns the placeholder to write into the SQL, so that a composer cannot push a value
/// and forget to reference it, or reference an index it never bound.
pub(super) struct Params<'d> {
    dialect: &'d dyn Dialect,
    values: Vec<DataValue>,
}

impl<'d> Params<'d> {
    pub(super) fn new(dialect: &'d dyn Dialect) -> Self {
        Self {
            dialect,
            values: Vec::new(),
        }
    }

    pub(super) fn dialect(&self) -> &'d dyn Dialect {
        self.dialect
    }

    /// Bind a value and return the placeholder that names it.
    pub(super) fn bind(&mut self, value: DataValue) -> String {
        self.values.push(value);
        self.dialect.placeholder(self.values.len())
    }

    pub(super) fn into_values(self) -> Vec<DataValue> {
        self.values
    }
}
