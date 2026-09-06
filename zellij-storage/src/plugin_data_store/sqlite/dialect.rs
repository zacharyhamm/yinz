//! How SQLite spells the things the engines disagree on.

use zellij_types::plugin_data::{AttrKind, DataError, DataValue};

use crate::plugin_data_store::dialect::{Dialect, SearchSql};
use crate::plugin_data_store::schema::EntitySchema;
use crate::plugin_data_store::sql::{join_idents, quote_ident};
use crate::plugin_data_store::value::{common_literal, hex};

pub(crate) struct SqliteDialect;

pub(crate) static SQLITE: SqliteDialect = SqliteDialect;

impl Dialect for SqliteDialect {
    fn index_name(&self, name: &str) -> String {
        name.to_owned()
    }

    fn placeholder(&self, index: usize) -> String {
        format!("?{}", index)
    }

    fn column_type(&self, kind: AttrKind) -> &'static str {
        kind.sql_type()
    }

    fn store_columns(&self) -> [String; 5] {
        [
            "id INTEGER PRIMARY KEY AUTOINCREMENT".to_owned(),
            "uid TEXT UNIQUE".to_owned(),
            "created_at INTEGER NOT NULL".to_owned(),
            "updated_at INTEGER NOT NULL".to_owned(),
            "rev INTEGER NOT NULL DEFAULT 1".to_owned(),
        ]
    }

    fn rev_column_added(&self) -> &'static str {
        "INTEGER NOT NULL DEFAULT 1"
    }

    fn registry_ddl(&self) -> &'static str {
        r#"CREATE TABLE IF NOT EXISTS store_entities (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               namespace TEXT NOT NULL,
               entity TEXT NOT NULL,
               version INTEGER NOT NULL,
               definition TEXT NOT NULL,
               owner_plugin TEXT NOT NULL,
               UNIQUE(namespace, entity)
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
            // SQLite has no boolean type; a Bool column is an INTEGER holding 0 or 1.
            DataValue::Bool(b) => (*b as i64).to_string(),
            DataValue::Blob(b) => format!("X'{}'", hex(b)),
            _ => unreachable!("common_literal handles every other kind"),
        })
    }

    /// SQLite's `LIKE` is ASCII-case-insensitive by default, which is the behaviour the store
    /// promises. The escape character has to be named; there is no default.
    fn like(&self, column: &str, placeholder: &str) -> String {
        format!("{} LIKE {} ESCAPE '\\'", column, placeholder)
    }

    /// An external-content FTS5 table kept exact by triggers: the FTS table holds only the index
    /// and reads the columns back from the entity table, so the text is not stored twice.
    fn full_text_create(&self, schema: &EntitySchema, attrs: &[&str]) -> String {
        let table = schema.table();
        // The same column list three times over: bare for the table and the trigger targets, and
        // behind `new.`/`old.` for the values the triggers read out of the row.
        let quoted = join_idents("", attrs);
        let new_values = join_idents("new.", attrs);
        let old_values = join_idents("old.", attrs);
        let [ai, ad, au] = schema.fts_triggers().map(|t| quote_ident(&t));
        let fts_q = quote_ident(&schema.fts_table());
        let table_q = quote_ident(&table);
        // `content=` takes the table name as a string literal rather than an identifier, so that
        // one stays unquoted.
        format!(
            "CREATE VIRTUAL TABLE {fts_q} USING fts5(
                 {quoted}, content='{table}', content_rowid='id', tokenize='unicode61'
             );
             CREATE TRIGGER {ai} AFTER INSERT ON {table_q} BEGIN
                 INSERT INTO {fts_q}(rowid, {quoted}) VALUES (new.id, {new_values});
             END;
             CREATE TRIGGER {ad} AFTER DELETE ON {table_q} BEGIN
                 INSERT INTO {fts_q}({fts_q}, rowid, {quoted})
                     VALUES('delete', old.id, {old_values});
             END;
             CREATE TRIGGER {au} AFTER UPDATE ON {table_q} BEGIN
                 INSERT INTO {fts_q}({fts_q}, rowid, {quoted})
                     VALUES('delete', old.id, {old_values});
                 INSERT INTO {fts_q}(rowid, {quoted}) VALUES (new.id, {new_values});
             END;
             INSERT INTO {fts_q}({fts_q}) VALUES('rebuild');"
        )
    }

    fn full_text_drop(&self, schema: &EntitySchema) -> String {
        let [ai, ad, au] = schema.fts_triggers().map(|t| quote_ident(&t));
        let fts = quote_ident(&schema.fts_table());
        format!(
            "DROP TRIGGER IF EXISTS {ai};
             DROP TRIGGER IF EXISTS {ad};
             DROP TRIGGER IF EXISTS {au};
             DROP TABLE IF EXISTS {fts};"
        )
    }

    fn search(&self, schema: &EntitySchema, placeholder: &str) -> SearchSql {
        let fts = quote_ident(&schema.fts_table());
        let table = quote_ident(&schema.table());
        SearchSql {
            join: format!(" JOIN {fts} ON {fts}.rowid = {table}.\"id\""),
            condition: format!("{fts} MATCH {placeholder}"),
            // bm25 returns a more negative score the better the match, so ascending is best-first.
            rank: format!("bm25({fts}) ASC"),
        }
    }

    fn retries_search_as_phrase(&self) -> bool {
        true
    }
}
