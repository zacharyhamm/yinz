//! How Postgres spells the things the engines disagree on.

use sha2::{Digest, Sha256};

use crate::plugin_data::{AttrKind, DataError, DataValue};

use crate::plugin_data_store::dialect::{Dialect, SearchSql};
use crate::plugin_data_store::schema::EntitySchema;
use crate::plugin_data_store::sql::{qualify, quote_ident};
use crate::plugin_data_store::value::{common_literal, hex};

pub(crate) struct PostgresDialect;

pub(crate) static POSTGRES: PostgresDialect = PostgresDialect;

/// The text-search configuration for the full-text index: lowercases and splits on non-word
/// characters, no stemming and no stop words. The nearest thing to FTS5's `unicode61`, and
/// immutable, which a generated column requires of its expression.
const TS_CONFIG: &str = "'simple'";

impl Dialect for PostgresDialect {
    fn index_name(&self, name: &str) -> String {
        if name.len() <= 63 {
            name.to_owned()
        } else {
            format!("{}_{}", &name[..30], hex(&Sha256::digest(name)[..16]))
        }
    }

    fn placeholder(&self, index: usize) -> String {
        format!("${}", index)
    }

    /// Text carries `COLLATE "C"` so that comparison and ordering are byte-wise, as they are on
    /// SQLite — a plugin's `ORDER BY uid` has to mean the same thing on both, and the database's
    /// own collation is whatever locale it was created under.
    fn column_type(&self, kind: AttrKind) -> &'static str {
        match kind {
            AttrKind::Text | AttrKind::Json => "TEXT COLLATE \"C\"",
            AttrKind::Int => "BIGINT",
            AttrKind::Real => "DOUBLE PRECISION",
            AttrKind::Bool => "BOOLEAN",
            AttrKind::Blob => "BYTEA",
        }
    }

    fn store_columns(&self) -> [String; 5] {
        [
            "id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY".to_owned(),
            "uid TEXT COLLATE \"C\" UNIQUE".to_owned(),
            "created_at BIGINT NOT NULL".to_owned(),
            "updated_at BIGINT NOT NULL".to_owned(),
            "rev BIGINT NOT NULL DEFAULT 1".to_owned(),
        ]
    }

    fn rev_column_added(&self) -> &'static str {
        "BIGINT NOT NULL DEFAULT 1"
    }

    fn registry_ddl(&self) -> &'static str {
        r#"CREATE TABLE IF NOT EXISTS store_entities (
               id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
               namespace TEXT COLLATE "C" NOT NULL,
               entity TEXT COLLATE "C" NOT NULL,
               version BIGINT NOT NULL,
               definition TEXT NOT NULL,
               owner_plugin TEXT NOT NULL,
               UNIQUE (namespace, entity)
           );"#
    }

    fn store_meta_ddl(&self) -> &'static str {
        "CREATE TABLE IF NOT EXISTS store_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);"
    }

    fn literal(&self, value: &DataValue) -> Result<String, DataError> {
        if let Some(common) = common_literal(value) {
            return common;
        }
        Ok(match value {
            DataValue::Bool(true) => "TRUE".to_owned(),
            DataValue::Bool(false) => "FALSE".to_owned(),
            DataValue::Blob(b) => format!("'\\x{}'::bytea", hex(b)),
            _ => unreachable!("common_literal handles every other kind"),
        })
    }

    /// `ILIKE` on a `COLLATE "C"` column folds ASCII case and nothing else, which is exactly what
    /// SQLite's `LIKE` does. Backslash is Postgres's default escape character, so the pattern
    /// needs no `ESCAPE` clause.
    fn like(&self, column: &str, placeholder: &str) -> String {
        format!("{} ILIKE {}", column, placeholder)
    }

    /// A stored generated `tsvector` column with a GIN index over it. Generated, so that Postgres
    /// keeps it exact on every write and backfills it for the rows already stored — the work
    /// SQLite needs three triggers and a rebuild for.
    fn full_text_create(&self, schema: &EntitySchema, attrs: &[&str]) -> String {
        let table = quote_ident(&schema.table());
        let column = quote_ident(schema.fts_column());
        let index = quote_ident(&schema.fts_index());
        let text = attrs
            .iter()
            .map(|attr| format!("coalesce({}, '')", quote_ident(attr)))
            .collect::<Vec<_>>()
            .join(" || ' ' || ");
        format!(
            "ALTER TABLE {table} ADD COLUMN {column} tsvector \
             GENERATED ALWAYS AS (to_tsvector({TS_CONFIG}, {text})) STORED;
             CREATE INDEX {index} ON {table} USING GIN ({column});"
        )
    }

    /// Dropping the column takes its index with it.
    fn full_text_drop(&self, schema: &EntitySchema) -> String {
        format!(
            "ALTER TABLE {} DROP COLUMN IF EXISTS {};",
            quote_ident(&schema.table()),
            quote_ident(schema.fts_column())
        )
    }

    fn search(&self, schema: &EntitySchema, placeholder: &str) -> SearchSql {
        let column = qualify(&schema.table(), schema.fts_column());
        let query = format!("websearch_to_tsquery({TS_CONFIG}, {placeholder})");
        SearchSql {
            join: String::new(),
            condition: format!("{column} @@ {query}"),
            rank: format!("ts_rank_cd({column}, {query}) DESC"),
        }
    }

    /// `websearch_to_tsquery` accepts any string, so there is never a syntax error to retry.
    fn retries_search_as_phrase(&self) -> bool {
        false
    }
}
