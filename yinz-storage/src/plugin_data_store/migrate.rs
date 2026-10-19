//! Bringing a stored entity's table in line with a new definition.

use yinz_types::plugin_data::{AttrDef, DataError, DataSchemaInfo, EntityDef};

use super::ddl::{create_attr_indexes, drop_attr_indexes, drop_full_text, rebuild_full_text};
use super::executor::Executor;
use super::registry::{column_definition, schema_info, store_definition};
use super::schema::EntitySchema;
use super::sql::{quote_ident, StoreContext};

pub(super) fn migrate_entity(
    exec: &mut dyn Executor,
    namespace: &str,
    owner_plugin: &str,
    existing: &EntitySchema,
    def: &EntityDef,
    allow_destructive: bool,
) -> Result<DataSchemaInfo, DataError> {
    // In a shared namespace a second plugin may extend an entity but never shrink it: the plugin
    // that declared it is still reading the columns this one would drop. Conflicts are an error
    // rather than a silent migration in either direction.
    let foreign_owner = existing.owner_plugin != owner_plugin;
    if foreign_owner && allow_destructive {
        return Err(DataError::RefusedMigration(format!(
            "`{}` in namespace `{}` was declared by `{}`; only its owner may make destructive \
             changes",
            def.name, namespace, existing.owner_plugin
        )));
    }
    if foreign_owner {
        for stored in &existing.def.attrs {
            if def.attr_named(&stored.name).is_some_and(|attr| {
                (stored.unique && !attr.unique)
                    || (stored.full_text && !attr.full_text)
                    || (stored.indexed && !attr.indexed)
            }) {
                return Err(DataError::RefusedMigration(format!(
                    "only `{}` may remove indexes from `{}` of `{}`",
                    existing.owner_plugin, stored.name, def.name
                )));
            }
        }
    }

    let mut info = schema_info(namespace, def, false);

    // Work out everything that is about to change, and refuse the whole migration, before
    // performing any of it — a half-applied schema change is worse than a rejected one.
    let removed: Vec<&AttrDef> = existing
        .def
        .attrs
        .iter()
        .filter(|stored| def.attr_named(&stored.name).is_none())
        .collect();
    if !removed.is_empty() {
        let names = removed
            .iter()
            .map(|a| format!("`{}`", a.name))
            .collect::<Vec<_>>()
            .join(", ");
        if !allow_destructive {
            return Err(DataError::RefusedMigration(format!(
                "the new definition of `{}` drops {}, which would discard the stored values; pass \
                 allow_destructive to confirm, or leave the attributes declared and stop reading \
                 them",
                def.name, names
            )));
        }
        if def.version <= existing.def.version {
            return Err(DataError::RefusedMigration(format!(
                "dropping {} from `{}` needs a version above the stored {}, so that the change is \
                 recorded rather than silent",
                names, def.name, existing.def.version
            )));
        }
    }

    let stored_ft: Vec<&str> = existing
        .def
        .full_text_attrs()
        .into_iter()
        .map(|a| a.name.as_str())
        .collect();
    let new_ft: Vec<&str> = def
        .full_text_attrs()
        .into_iter()
        .map(|a| a.name.as_str())
        .collect();
    // The full-text index names the columns it covers, so it has to be rebuilt whenever that set
    // changes. It also has to come down *before* any column is dropped, because SQLite
    // refuse to drop a column it still names.
    let rebuild_needed = stored_ft != new_ft || !removed.is_empty();
    if rebuild_needed && !stored_ft.is_empty() {
        drop_full_text(exec, existing)?;
    }

    let dialect = exec.dialect();
    for attr in &def.attrs {
        match existing.attr(&attr.name) {
            Some(stored) if stored.kind != attr.kind => {
                return Err(DataError::RefusedMigration(format!(
                    "attribute `{}` of `{}` is stored as {:?} and the new definition makes it \
                     {:?}; changing the type of a column would have to reinterpret existing rows, \
                     so declare a new attribute instead",
                    attr.name, def.name, stored.kind, attr.kind
                )));
            },
            Some(stored) if stored.required != attr.required || stored.default != attr.default => {
                return Err(DataError::RefusedMigration(format!(
                    "attribute `{}` of `{}` changes required or default; changing these column \
                     constraints is not supported, so declare a new attribute instead",
                    attr.name, def.name
                )));
            },
            Some(stored) => {
                if attr.indexed != stored.indexed || attr.unique != stored.unique {
                    drop_attr_indexes(exec, existing, &attr.name)?;
                    if create_attr_indexes(exec, existing, attr)? {
                        info.indexes_created.push(attr.name.clone());
                    }
                }
            },
            None => {
                if attr.required && attr.default.is_none() {
                    return Err(DataError::RefusedMigration(format!(
                        "attribute `{}` is being added to the existing entity `{}` as required, \
                         but has no default; the store cannot add a NOT NULL column without one, \
                         and the rows already stored would have nothing to put in it",
                        attr.name, def.name
                    )));
                }
                exec.execute_batch(&format!(
                    "ALTER TABLE {} ADD COLUMN {};",
                    quote_ident(&existing.table()),
                    column_definition(dialect, attr)?
                ))
                .with_context(|| format!("failed to add `{}`", attr.name))?;
                info.attrs_added.push(attr.name.clone());
                if create_attr_indexes(exec, existing, attr)? {
                    info.indexes_created.push(attr.name.clone());
                }
            },
        }
    }

    for attr in removed {
        drop_attr_indexes(exec, existing, &attr.name)?;
        exec.execute_batch(&format!(
            "ALTER TABLE {} DROP COLUMN {};",
            quote_ident(&existing.table()),
            quote_ident(&attr.name)
        ))
        .with_context(|| format!("failed to drop `{}`", attr.name))?;
    }

    let updated = EntitySchema {
        row_id: existing.row_id,
        def: def.clone(),
        owner_plugin: existing.owner_plugin.clone(),
    };
    if rebuild_needed {
        info.full_text_rebuilt = rebuild_full_text(exec, &updated)?;
    }

    store_definition(exec, existing.row_id, def)?;
    Ok(info)
}
