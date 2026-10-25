//! Conversions between the data store's protobuf types and the native ones in
//! [`yinz_types::plugin_data`].
//!
//! Kept in its own module rather than in `plugin_command.rs` because `event.rs` needs the row
//! conversions too, for the change notification.
//!
//! [`Pred`] is recursive, so both directions carry a depth counter and refuse a tree deeper than
//! [`MAX_PRED_DEPTH`]. Without it, a plugin could send a structure deep enough to exhaust the host's
//! stack while it was being decoded — the conversion runs before any of the validation in
//! `plugin_data` gets a chance to look at the shape.

use super::generated_api::api::plugin_data::{
    mutation::Mutation as ProtobufMutationOneof, pred::Pred as ProtobufPredOneof,
    value::Value as ProtobufValueOneof,
};
pub use super::generated_api::api::plugin_data::{
    ApplyReport as ProtobufApplyReport, AssertRows as ProtobufAssertRows,
    AttrDef as ProtobufAttrDef, AttrKind as ProtobufAttrKind, AttrPattern as ProtobufAttrPattern,
    AttrValue as ProtobufAttrValue, AttrValues as ProtobufAttrValues, Delete as ProtobufDelete,
    EntityDef as ProtobufEntityDef, EntityInfo as ProtobufEntityInfo,
    FetchRequest as ProtobufFetchRequest, Mutation as ProtobufMutation, Pred as ProtobufPred,
    PredList as ProtobufPredList, Put as ProtobufPut, Row as ProtobufRow,
    RowRevision as ProtobufRowRevision, Rows as ProtobufRows, SchemaInfo as ProtobufSchemaInfo,
    Sort as ProtobufSort, SortDir as ProtobufSortDir, Update as ProtobufUpdate,
    Value as ProtobufValue, WatchHandle as ProtobufWatchHandle, WatchMode as ProtobufWatchMode,
};
use yinz_types::internal_ids::DataRevision;
use yinz_types::internal_ids::DataRowId;
use yinz_types::internal_ids::DataWatchId;

use {
    yinz_types::data::ApplyReport, yinz_types::data::AttrDef, yinz_types::data::AttrKind,
    yinz_types::data::DataEntityInfo, yinz_types::data::DataRows, yinz_types::data::DataSchemaInfo,
    yinz_types::data::DataValue, yinz_types::data::DataWatchHandle, yinz_types::data::EntityDef,
    yinz_types::data::FetchRequest, yinz_types::data::Mutation, yinz_types::data::Pred,
    yinz_types::data::SortDir, yinz_types::data::WatchMode, yinz_types::data::MAX_PRED_DEPTH,
};

use std::convert::TryFrom;

// ------------------------------------------------------------------- values

impl From<DataValue> for ProtobufValue {
    fn from(value: DataValue) -> Self {
        ProtobufValue {
            // Null is the absent oneof rather than a branch, so it needs no variant here.
            value: match value {
                DataValue::Null => None,
                DataValue::Int(i) => Some(ProtobufValueOneof::IntValue(i)),
                DataValue::Real(f) => Some(ProtobufValueOneof::RealValue(f)),
                DataValue::Text(s) => Some(ProtobufValueOneof::TextValue(s)),
                DataValue::Bool(b) => Some(ProtobufValueOneof::BoolValue(b)),
                DataValue::Blob(b) => Some(ProtobufValueOneof::BlobValue(b)),
            },
        }
    }
}

impl From<ProtobufValue> for DataValue {
    fn from(value: ProtobufValue) -> Self {
        match value.value {
            None => DataValue::Null,
            Some(ProtobufValueOneof::IntValue(i)) => DataValue::Int(i),
            Some(ProtobufValueOneof::RealValue(f)) => DataValue::Real(f),
            Some(ProtobufValueOneof::TextValue(s)) => DataValue::Text(s),
            Some(ProtobufValueOneof::BoolValue(b)) => DataValue::Bool(b),
            Some(ProtobufValueOneof::BlobValue(b)) => DataValue::Blob(b),
        }
    }
}

// -------------------------------------------------------------------- enums

// The proto3 enum values carry their type name as a prefix, because proto3 scopes them to the
// package; prost strips it back off, so the Rust variants below are the bare names.
impl From<AttrKind> for ProtobufAttrKind {
    fn from(kind: AttrKind) -> Self {
        match kind {
            AttrKind::Text => ProtobufAttrKind::Text,
            AttrKind::Int => ProtobufAttrKind::Int,
            AttrKind::Real => ProtobufAttrKind::Real,
            AttrKind::Bool => ProtobufAttrKind::Bool,
            AttrKind::Blob => ProtobufAttrKind::Blob,
            AttrKind::Json => ProtobufAttrKind::Json,
        }
    }
}

impl From<ProtobufAttrKind> for AttrKind {
    fn from(kind: ProtobufAttrKind) -> Self {
        match kind {
            ProtobufAttrKind::Text => AttrKind::Text,
            ProtobufAttrKind::Int => AttrKind::Int,
            ProtobufAttrKind::Real => AttrKind::Real,
            ProtobufAttrKind::Bool => AttrKind::Bool,
            ProtobufAttrKind::Blob => AttrKind::Blob,
            ProtobufAttrKind::Json => AttrKind::Json,
        }
    }
}

impl From<SortDir> for ProtobufSortDir {
    fn from(dir: SortDir) -> Self {
        match dir {
            SortDir::Asc => ProtobufSortDir::Asc,
            SortDir::Desc => ProtobufSortDir::Desc,
        }
    }
}

impl From<ProtobufSortDir> for SortDir {
    fn from(dir: ProtobufSortDir) -> Self {
        match dir {
            ProtobufSortDir::Asc => SortDir::Asc,
            ProtobufSortDir::Desc => SortDir::Desc,
        }
    }
}

impl From<WatchMode> for ProtobufWatchMode {
    fn from(mode: WatchMode) -> Self {
        match mode {
            WatchMode::Rows => ProtobufWatchMode::Rows,
            WatchMode::Ids => ProtobufWatchMode::Ids,
        }
    }
}

impl From<ProtobufWatchMode> for WatchMode {
    fn from(mode: ProtobufWatchMode) -> Self {
        match mode {
            ProtobufWatchMode::Rows => WatchMode::Rows,
            ProtobufWatchMode::Ids => WatchMode::Ids,
        }
    }
}

/// Decode a proto3 enum, which arrives as a bare `i32`.
///
/// An unrecognised value is rejected rather than defaulted: it means the plugin was built against a
/// newer `yinz-tile` than the host, and silently treating an unknown kind as `Text` would corrupt
/// whatever it stored.
pub fn attr_kind_from_i32(kind: i32) -> Result<AttrKind, &'static str> {
    ProtobufAttrKind::try_from(kind)
        .map(AttrKind::from)
        .map_err(|_| "Unrecognized attribute kind")
}

pub fn sort_dir_from_i32(dir: i32) -> Result<SortDir, &'static str> {
    ProtobufSortDir::try_from(dir)
        .map(SortDir::from)
        .map_err(|_| "Unrecognized sort direction")
}

pub fn watch_mode_from_i32(mode: i32) -> Result<WatchMode, &'static str> {
    ProtobufWatchMode::try_from(mode)
        .map(WatchMode::from)
        .map_err(|_| "Unrecognized watch mode")
}

// ------------------------------------------------------------------- schema

impl From<AttrDef> for ProtobufAttrDef {
    fn from(attr: AttrDef) -> Self {
        ProtobufAttrDef {
            name: attr.name,
            kind: ProtobufAttrKind::from(attr.kind) as i32,
            indexed: attr.indexed,
            full_text: attr.full_text,
            unique: attr.unique,
            required: attr.required,
            default: attr.default.map(ProtobufValue::from),
        }
    }
}

impl TryFrom<ProtobufAttrDef> for AttrDef {
    type Error = &'static str;
    fn try_from(attr: ProtobufAttrDef) -> Result<Self, &'static str> {
        Ok(AttrDef {
            name: attr.name,
            kind: attr_kind_from_i32(attr.kind)?,
            indexed: attr.indexed,
            full_text: attr.full_text,
            unique: attr.unique,
            required: attr.required,
            default: attr.default.map(DataValue::from),
        })
    }
}

impl From<EntityDef> for ProtobufEntityDef {
    fn from(def: EntityDef) -> Self {
        ProtobufEntityDef {
            name: def.name,
            version: def.version,
            attrs: def.attrs.into_iter().map(ProtobufAttrDef::from).collect(),
        }
    }
}

impl TryFrom<ProtobufEntityDef> for EntityDef {
    type Error = &'static str;
    fn try_from(def: ProtobufEntityDef) -> Result<Self, &'static str> {
        Ok(EntityDef {
            name: def.name,
            version: def.version,
            attrs: def
                .attrs
                .into_iter()
                .map(AttrDef::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }
}

impl From<DataEntityInfo> for ProtobufEntityInfo {
    fn from(info: DataEntityInfo) -> Self {
        ProtobufEntityInfo {
            namespace: info.namespace,
            owner_plugin: info.owner_plugin,
            def: Some(ProtobufEntityDef::from(info.def)),
        }
    }
}

impl TryFrom<ProtobufEntityInfo> for DataEntityInfo {
    type Error = &'static str;
    fn try_from(info: ProtobufEntityInfo) -> Result<Self, &'static str> {
        Ok(DataEntityInfo {
            namespace: info.namespace,
            owner_plugin: info.owner_plugin,
            def: EntityDef::try_from(info.def.ok_or("EntityInfo without a definition")?)?,
        })
    }
}

// --------------------------------------------------------------- predicates

fn attr_value(attr: String, value: DataValue) -> ProtobufAttrValue {
    ProtobufAttrValue {
        attr,
        value: Some(ProtobufValue::from(value)),
    }
}

fn attr_pattern(attr: String, pattern: String) -> ProtobufAttrPattern {
    ProtobufAttrPattern { attr, pattern }
}

/// Take an `AttrValue` apart, defaulting an absent value to null rather than failing: proto3 cannot
/// distinguish "no value set" from "null was set", and null is what an unset value means.
fn split_attr_value(pair: ProtobufAttrValue) -> (String, DataValue) {
    (
        pair.attr,
        pair.value.map(DataValue::from).unwrap_or(DataValue::Null),
    )
}

fn pred_to_protobuf(pred: Pred, depth: usize) -> Result<ProtobufPred, &'static str> {
    if depth > MAX_PRED_DEPTH {
        return Err("Predicate is nested too deeply");
    }
    let inner = match pred {
        Pred::Eq(a, v) => ProtobufPredOneof::Eq(attr_value(a, v)),
        Pred::Ne(a, v) => ProtobufPredOneof::Ne(attr_value(a, v)),
        Pred::Gt(a, v) => ProtobufPredOneof::Gt(attr_value(a, v)),
        Pred::Gte(a, v) => ProtobufPredOneof::Gte(attr_value(a, v)),
        Pred::Lt(a, v) => ProtobufPredOneof::Lt(attr_value(a, v)),
        Pred::Lte(a, v) => ProtobufPredOneof::Lte(attr_value(a, v)),
        Pred::In(a, values) => ProtobufPredOneof::InList(ProtobufAttrValues {
            attr: a,
            values: values.into_iter().map(ProtobufValue::from).collect(),
        }),
        Pred::IsNull(a) => ProtobufPredOneof::IsNull(a),
        Pred::NotNull(a) => ProtobufPredOneof::NotNull(a),
        Pred::Prefix(a, v) => ProtobufPredOneof::Prefix(attr_pattern(a, v)),
        Pred::Contains(a, v) => ProtobufPredOneof::Contains(attr_pattern(a, v)),
        Pred::And(preds) => ProtobufPredOneof::And(pred_list_to_protobuf(preds, depth)?),
        Pred::Or(preds) => ProtobufPredOneof::Or(pred_list_to_protobuf(preds, depth)?),
        Pred::Not(pred) => ProtobufPredOneof::Not(Box::new(pred_to_protobuf(*pred, depth + 1)?)),
    };
    Ok(ProtobufPred { pred: Some(inner) })
}

fn pred_list_to_protobuf(preds: Vec<Pred>, depth: usize) -> Result<ProtobufPredList, &'static str> {
    Ok(ProtobufPredList {
        preds: preds
            .into_iter()
            .map(|p| pred_to_protobuf(p, depth + 1))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn pred_from_protobuf(pred: ProtobufPred, depth: usize) -> Result<Pred, &'static str> {
    if depth > MAX_PRED_DEPTH {
        return Err("Predicate is nested too deeply");
    }
    let Some(inner) = pred.pred else {
        return Err("Predicate has no branch set");
    };
    Ok(match inner {
        ProtobufPredOneof::Eq(p) => {
            let (a, v) = split_attr_value(p);
            Pred::Eq(a, v)
        },
        ProtobufPredOneof::Ne(p) => {
            let (a, v) = split_attr_value(p);
            Pred::Ne(a, v)
        },
        ProtobufPredOneof::Gt(p) => {
            let (a, v) = split_attr_value(p);
            Pred::Gt(a, v)
        },
        ProtobufPredOneof::Gte(p) => {
            let (a, v) = split_attr_value(p);
            Pred::Gte(a, v)
        },
        ProtobufPredOneof::Lt(p) => {
            let (a, v) = split_attr_value(p);
            Pred::Lt(a, v)
        },
        ProtobufPredOneof::Lte(p) => {
            let (a, v) = split_attr_value(p);
            Pred::Lte(a, v)
        },
        ProtobufPredOneof::InList(p) => {
            Pred::In(p.attr, p.values.into_iter().map(DataValue::from).collect())
        },
        ProtobufPredOneof::IsNull(a) => Pred::IsNull(a),
        ProtobufPredOneof::NotNull(a) => Pred::NotNull(a),
        ProtobufPredOneof::Prefix(p) => Pred::Prefix(p.attr, p.pattern),
        ProtobufPredOneof::Contains(p) => Pred::Contains(p.attr, p.pattern),
        ProtobufPredOneof::And(list) => Pred::And(pred_list_from_protobuf(list, depth)?),
        ProtobufPredOneof::Or(list) => Pred::Or(pred_list_from_protobuf(list, depth)?),
        ProtobufPredOneof::Not(pred) => Pred::Not(Box::new(pred_from_protobuf(*pred, depth + 1)?)),
    })
}

fn pred_list_from_protobuf(
    list: ProtobufPredList,
    depth: usize,
) -> Result<Vec<Pred>, &'static str> {
    list.preds
        .into_iter()
        .map(|p| pred_from_protobuf(p, depth + 1))
        .collect()
}

impl TryFrom<Pred> for ProtobufPred {
    type Error = &'static str;
    fn try_from(pred: Pred) -> Result<Self, &'static str> {
        pred_to_protobuf(pred, 1)
    }
}

impl TryFrom<ProtobufPred> for Pred {
    type Error = &'static str;
    fn try_from(pred: ProtobufPred) -> Result<Self, &'static str> {
        pred_from_protobuf(pred, 1)
    }
}

// ------------------------------------------------------------------ fetches

impl TryFrom<FetchRequest> for ProtobufFetchRequest {
    type Error = &'static str;
    fn try_from(request: FetchRequest) -> Result<Self, &'static str> {
        Ok(ProtobufFetchRequest {
            namespace: request.namespace,
            entity: request.entity,
            filter: request.filter.map(ProtobufPred::try_from).transpose()?,
            search: request.search,
            sort: request
                .sort
                .into_iter()
                .map(|(attr, dir)| ProtobufSort {
                    attr,
                    dir: ProtobufSortDir::from(dir) as i32,
                })
                .collect(),
            select: request.select,
            limit: request.limit,
            offset: request.offset,
            after_id: request.after_id.map(|id| id.get()),
        })
    }
}

impl TryFrom<ProtobufFetchRequest> for FetchRequest {
    type Error = &'static str;
    fn try_from(request: ProtobufFetchRequest) -> Result<Self, &'static str> {
        Ok(FetchRequest {
            namespace: request.namespace,
            entity: request.entity,
            filter: request.filter.map(Pred::try_from).transpose()?,
            search: request.search,
            sort: request
                .sort
                .into_iter()
                .map(|s| Ok((s.attr, sort_dir_from_i32(s.dir)?)))
                .collect::<Result<Vec<_>, &'static str>>()?,
            select: request.select,
            limit: request.limit,
            offset: request.offset,
            after_id: request.after_id.map(DataRowId::new),
        })
    }
}

// ---------------------------------------------------------------- mutations

impl TryFrom<Mutation> for ProtobufMutation {
    type Error = &'static str;
    fn try_from(mutation: Mutation) -> Result<Self, &'static str> {
        let inner = match mutation {
            Mutation::AssertRows {
                entity,
                filter,
                rows,
            } => ProtobufMutationOneof::AssertRows(ProtobufAssertRows {
                entity,
                filter: Some(ProtobufPred::try_from(filter)?),
                rows: rows
                    .into_iter()
                    .map(|(id, rev)| ProtobufRowRevision {
                        id: id.get(),
                        rev: rev.get(),
                    })
                    .collect(),
            }),
            Mutation::Put {
                entity,
                uid,
                values,
                expect_rev,
            } => ProtobufMutationOneof::Put(ProtobufPut {
                entity,
                uid,
                expect_rev: expect_rev.map(|id| id.get()),
                values: values
                    .into_iter()
                    .map(|(attr, value)| attr_value(attr, value))
                    .collect(),
            }),
            Mutation::Update {
                entity,
                filter,
                values,
                expect_rev,
            } => ProtobufMutationOneof::Update(ProtobufUpdate {
                entity,
                filter: Some(ProtobufPred::try_from(filter)?),
                expect_rev: expect_rev.map(|id| id.get()),
                values: values
                    .into_iter()
                    .map(|(attr, value)| attr_value(attr, value))
                    .collect(),
            }),
            Mutation::Delete { entity, filter } => ProtobufMutationOneof::Delete(ProtobufDelete {
                entity,
                filter: Some(ProtobufPred::try_from(filter)?),
            }),
        };
        Ok(ProtobufMutation {
            mutation: Some(inner),
        })
    }
}

impl TryFrom<ProtobufMutation> for Mutation {
    type Error = &'static str;
    fn try_from(mutation: ProtobufMutation) -> Result<Self, &'static str> {
        let Some(inner) = mutation.mutation else {
            return Err("Mutation has no branch set");
        };
        Ok(match inner {
            ProtobufMutationOneof::AssertRows(assertion) => Mutation::AssertRows {
                entity: assertion.entity,
                filter: assertion
                    .filter
                    .map(Pred::try_from)
                    .transpose()?
                    .ok_or("AssertRows has no filter")?,
                rows: assertion
                    .rows
                    .into_iter()
                    .map(|row| (DataRowId::new(row.id), DataRevision::new(row.rev)))
                    .collect(),
            },
            ProtobufMutationOneof::Put(put) => Mutation::Put {
                entity: put.entity,
                uid: put.uid,
                values: put.values.into_iter().map(split_attr_value).collect(),
                expect_rev: put.expect_rev.map(DataRevision::new),
            },
            ProtobufMutationOneof::Update(update) => Mutation::Update {
                entity: update.entity,
                filter: update
                    .filter
                    .map(Pred::try_from)
                    .transpose()?
                    .ok_or("Update has no filter")?,
                values: update.values.into_iter().map(split_attr_value).collect(),
                expect_rev: update.expect_rev.map(DataRevision::new),
            },
            ProtobufMutationOneof::Delete(delete) => Mutation::Delete {
                entity: delete.entity,
                filter: delete
                    .filter
                    .map(Pred::try_from)
                    .transpose()?
                    .ok_or("Delete has no filter")?,
            },
        })
    }
}

// --------------------------------------------------------------------- rows

impl From<DataRows> for ProtobufRows {
    fn from(rows: DataRows) -> Self {
        ProtobufRows {
            columns: rows.columns,
            rows: rows
                .rows
                .into_iter()
                .map(|values| ProtobufRow {
                    values: values.into_iter().map(ProtobufValue::from).collect(),
                })
                .collect(),
            truncated: rows.truncated,
            next_cursor: rows.next_cursor.map(|id| id.get()),
        }
    }
}

impl From<ProtobufRows> for DataRows {
    fn from(rows: ProtobufRows) -> Self {
        DataRows {
            columns: rows.columns,
            rows: rows
                .rows
                .into_iter()
                .map(|row| row.values.into_iter().map(DataValue::from).collect())
                .collect(),
            truncated: rows.truncated,
            next_cursor: rows.next_cursor.map(DataRowId::new),
        }
    }
}

// ------------------------------------------------------------------ reports

impl From<DataSchemaInfo> for ProtobufSchemaInfo {
    fn from(info: DataSchemaInfo) -> Self {
        ProtobufSchemaInfo {
            namespace: info.namespace,
            entity: info.entity,
            version: info.version,
            created: info.created,
            attrs_added: info.attrs_added,
            indexes_created: info.indexes_created,
            full_text_rebuilt: info.full_text_rebuilt,
        }
    }
}

impl From<ProtobufSchemaInfo> for DataSchemaInfo {
    fn from(info: ProtobufSchemaInfo) -> Self {
        DataSchemaInfo {
            namespace: info.namespace,
            entity: info.entity,
            version: info.version,
            created: info.created,
            attrs_added: info.attrs_added,
            indexes_created: info.indexes_created,
            full_text_rebuilt: info.full_text_rebuilt,
        }
    }
}

impl From<ApplyReport> for ProtobufApplyReport {
    fn from(report: ApplyReport) -> Self {
        ProtobufApplyReport {
            rows_written: report.rows_written,
            rows_deleted: report.rows_deleted,
            ids: report.ids.into_iter().map(|id| id.get()).collect(),
            per_mutation: report.per_mutation,
        }
    }
}

impl From<ProtobufApplyReport> for ApplyReport {
    fn from(report: ProtobufApplyReport) -> Self {
        ApplyReport {
            rows_written: report.rows_written,
            rows_deleted: report.rows_deleted,
            ids: report.ids.into_iter().map(DataRowId::new).collect(),
            per_mutation: report.per_mutation,
        }
    }
}

impl From<DataWatchHandle> for ProtobufWatchHandle {
    fn from(handle: DataWatchHandle) -> Self {
        ProtobufWatchHandle {
            watch_id: handle.watch_id.get(),
            rows: Some(ProtobufRows::from(handle.rows)),
        }
    }
}

impl From<ProtobufWatchHandle> for DataWatchHandle {
    fn from(handle: ProtobufWatchHandle) -> Self {
        DataWatchHandle {
            watch_id: DataWatchId::new(handle.watch_id),
            // An absent `rows` is an empty result set, which is what a watch over nothing returns
            // anyway — there is no useful way to fail here.
            rows: handle.rows.map(DataRows::from).unwrap_or_default(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
#[cfg_attr(not(test), allow(dead_code))]
#[path = "unit/plugin_data_tests.rs"]
pub(crate) mod tests;
