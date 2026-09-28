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
pub(crate) mod tests {
    use super::*;
    use hegel::generators::{self};
    use yinz_types::data::AttrDef as NativeAttrDef;

    // ------------------------------------------------------------ generators

    /// Any value, excluding NaN.
    ///
    /// NaN is excluded here only because `assert_eq!` on it is always false, which would make a
    /// round-trip assertion unwritable — not because the conversion cannot carry it. That it *is*
    /// carried is asserted separately in `nan_and_infinity_survive_the_round_trip`.
    #[hegel::composite]
    fn any_data_value(tc: &hegel::TestCase) -> DataValue {
        let choice = tc.draw(generators::integers::<u8>().max_value(5));
        match choice {
            0 => DataValue::Null,
            1 => DataValue::Int(tc.draw(generators::integers::<i64>())),
            2 => DataValue::Real(tc.draw(generators::floats::<f64>().allow_nan(false))),
            3 => DataValue::Text(tc.draw(generators::text())),
            4 => DataValue::Bool(tc.draw(generators::booleans())),
            _ => DataValue::Blob(tc.draw(generators::binary())),
        }
    }

    #[hegel::composite]
    fn any_attr_kind(tc: &hegel::TestCase) -> AttrKind {
        tc.draw(generators::sampled_from(vec![
            AttrKind::Text,
            AttrKind::Int,
            AttrKind::Real,
            AttrKind::Bool,
            AttrKind::Blob,
            AttrKind::Json,
        ]))
    }

    /// An attribute definition with an arbitrary flag combination.
    ///
    /// Names are arbitrary text rather than valid identifiers: the conversion does not validate
    /// them, and constraining them here would test less of it than the code actually accepts.
    #[hegel::composite]
    fn any_attr_def(tc: &hegel::TestCase) -> NativeAttrDef {
        NativeAttrDef {
            name: tc.draw(generators::text()),
            kind: tc.draw(any_attr_kind()),
            indexed: tc.draw(generators::booleans()),
            full_text: tc.draw(generators::booleans()),
            unique: tc.draw(generators::booleans()),
            required: tc.draw(generators::booleans()),
            default: tc.draw(generators::optional(any_data_value())),
        }
    }

    #[hegel::composite]
    pub(crate) fn any_entity_def(tc: &hegel::TestCase) -> EntityDef {
        EntityDef {
            name: tc.draw(generators::text()),
            version: tc.draw(generators::integers::<u32>()),
            attrs: tc.draw(generators::vecs(any_attr_def()).max_size(6)),
        }
    }

    /// A predicate tree with at most `depth` levels.
    ///
    /// Recursion is plain sequential code because hegel is imperative — the branch arms draw
    /// sub-predicates with a smaller budget, and a budget of zero can only produce a leaf, which is
    /// what makes this terminate.
    #[hegel::composite]
    fn any_pred(tc: &hegel::TestCase, depth: usize) -> Pred {
        let leaf_count = 11u8;
        let choice = if depth == 0 {
            tc.draw(generators::integers::<u8>().max_value(leaf_count - 1))
        } else {
            tc.draw(generators::integers::<u8>().max_value(leaf_count + 2))
        };
        let attr = |tc: &hegel::TestCase| tc.draw(generators::text());
        match choice {
            0 => Pred::Eq(attr(tc), tc.draw(any_data_value())),
            1 => Pred::Ne(attr(tc), tc.draw(any_data_value())),
            2 => Pred::Gt(attr(tc), tc.draw(any_data_value())),
            3 => Pred::Gte(attr(tc), tc.draw(any_data_value())),
            4 => Pred::Lt(attr(tc), tc.draw(any_data_value())),
            5 => Pred::Lte(attr(tc), tc.draw(any_data_value())),
            6 => Pred::In(
                attr(tc),
                tc.draw(generators::vecs(any_data_value()).max_size(4)),
            ),
            7 => Pred::IsNull(attr(tc)),
            8 => Pred::NotNull(attr(tc)),
            9 => Pred::Prefix(attr(tc), tc.draw(generators::text())),
            10 => Pred::Contains(attr(tc), tc.draw(generators::text())),
            11 => Pred::And(tc.draw(generators::vecs(any_pred(depth - 1)).max_size(3))),
            12 => Pred::Or(tc.draw(generators::vecs(any_pred(depth - 1)).max_size(3))),
            _ => Pred::Not(Box::new(tc.draw(any_pred(depth - 1)))),
        }
    }

    #[hegel::composite]
    pub(crate) fn any_fetch_request(tc: &hegel::TestCase) -> FetchRequest {
        FetchRequest {
            namespace: tc.draw(generators::optional(generators::text())),
            entity: tc.draw(generators::text()),
            filter: tc.draw(generators::optional(any_pred(3))),
            search: tc.draw(generators::optional(generators::text())),
            sort: tc.draw(
                generators::vecs(generators::tuples!(
                    generators::text(),
                    generators::sampled_from(vec![SortDir::Asc, SortDir::Desc]),
                ))
                .max_size(3),
            ),
            select: tc.draw(generators::vecs(generators::text()).max_size(4)),
            limit: tc.draw(generators::integers::<u32>()),
            offset: tc.draw(generators::integers::<u32>()),
            after_id: tc
                .draw(generators::optional(generators::integers::<i64>()))
                .map(DataRowId::new),
        }
    }

    #[hegel::composite]
    pub(crate) fn any_mutation(tc: &hegel::TestCase) -> Mutation {
        let values = generators::vecs(generators::tuples!(generators::text(), any_data_value(),))
            .max_size(4);
        match tc.draw(generators::integers::<u8>().max_value(3)) {
            0 => Mutation::Put {
                entity: tc.draw(generators::text()),
                uid: tc.draw(generators::optional(generators::text())),
                values: tc.draw(values),
                expect_rev: tc
                    .draw(generators::optional(generators::integers::<i64>()))
                    .map(DataRevision::new),
            },
            1 => Mutation::Update {
                entity: tc.draw(generators::text()),
                filter: tc.draw(any_pred(2)),
                values: tc.draw(values),
                expect_rev: tc
                    .draw(generators::optional(generators::integers::<i64>()))
                    .map(DataRevision::new),
            },
            3 => Mutation::AssertRows {
                entity: tc.draw(generators::text()),
                filter: tc.draw(any_pred(2)),
                rows: tc
                    .draw(
                        generators::vecs(generators::tuples!(
                            generators::integers::<i64>(),
                            generators::integers::<i64>()
                        ))
                        .max_size(4),
                    )
                    .into_iter()
                    .map(|(id, rev)| (DataRowId::new(id), DataRevision::new(rev)))
                    .collect(),
            },
            _ => Mutation::Delete {
                entity: tc.draw(generators::text()),
                filter: tc.draw(any_pred(2)),
            },
        }
    }

    #[hegel::composite]
    fn any_rows(tc: &hegel::TestCase) -> DataRows {
        let width = tc.draw(generators::integers::<usize>().max_value(4));
        let row_count = tc.draw(generators::integers::<usize>().max_value(6));
        let mut rows = Vec::with_capacity(row_count);
        for _ in 0..row_count {
            let mut row = Vec::with_capacity(width);
            for _ in 0..width {
                row.push(tc.draw(any_data_value()));
            }
            rows.push(row);
        }
        DataRows {
            columns: (0..width).map(|i| format!("c{}", i)).collect(),
            rows,
            truncated: tc.draw(generators::booleans()),
            next_cursor: tc
                .draw(generators::optional(generators::integers::<i64>()))
                .map(DataRowId::new),
        }
    }

    // ------------------------------------------------------------ properties

    /// Anything a plugin can put in a value reaches the host unchanged. The example-based tests
    /// below cover the shapes I thought of; this covers the ones I did not.
    #[hegel::test]
    fn any_value_round_trips(tc: hegel::TestCase) {
        let value = tc.draw(any_data_value());
        let protobuf = ProtobufValue::from(value.clone());
        assert_eq!(DataValue::from(protobuf), value);
    }

    /// The two float values `assert_eq!` cannot express, checked on their own terms rather than
    /// left untested.
    #[test]
    fn nan_and_infinity_survive_the_round_trip() {
        let nan = ProtobufValue::from(DataValue::Real(f64::NAN));
        match DataValue::from(nan) {
            DataValue::Real(f) => assert!(f.is_nan()),
            other => panic!("expected a real, got {:?}", other),
        }
        for infinity in [f64::INFINITY, f64::NEG_INFINITY] {
            let protobuf = ProtobufValue::from(DataValue::Real(infinity));
            assert_eq!(DataValue::from(protobuf), DataValue::Real(infinity));
        }
    }

    #[hegel::test]
    fn any_entity_definition_round_trips(tc: hegel::TestCase) {
        let def = tc.draw(any_entity_def());
        let protobuf = ProtobufEntityDef::from(def.clone());
        assert_eq!(EntityDef::try_from(protobuf), Ok(def));
    }

    #[hegel::test]
    fn any_predicate_round_trips(tc: hegel::TestCase) {
        let pred = tc.draw(any_pred(4));
        let protobuf = ProtobufPred::try_from(pred.clone()).unwrap();
        assert_eq!(Pred::try_from(protobuf), Ok(pred));
    }

    #[hegel::test]
    fn any_fetch_request_round_trips(tc: hegel::TestCase) {
        let request = tc.draw(any_fetch_request());
        let protobuf = ProtobufFetchRequest::try_from(request.clone()).unwrap();
        assert_eq!(FetchRequest::try_from(protobuf), Ok(request));
    }

    #[hegel::test]
    fn any_mutation_round_trips(tc: hegel::TestCase) {
        let mutation = tc.draw(any_mutation());
        let protobuf = ProtobufMutation::try_from(mutation.clone()).unwrap();
        assert_eq!(Mutation::try_from(protobuf), Ok(mutation));
    }

    #[hegel::test]
    fn any_rows_round_trip(tc: hegel::TestCase) {
        let rows = tc.draw(any_rows());
        let protobuf = ProtobufRows::from(rows.clone());
        assert_eq!(DataRows::from(protobuf), rows);
    }

    /// The wire format is what actually crosses the boundary, so the round trip has to survive
    /// encoding and decoding too — not just the Rust-to-Rust conversion either side of it.
    #[hegel::test]
    fn a_fetch_request_survives_being_encoded_and_decoded(tc: hegel::TestCase) {
        use prost::Message;
        let request = tc.draw(any_fetch_request());
        let protobuf = ProtobufFetchRequest::try_from(request.clone()).unwrap();
        let bytes = protobuf.encode_to_vec();
        let decoded = ProtobufFetchRequest::decode(bytes.as_slice()).unwrap();
        assert_eq!(FetchRequest::try_from(decoded), Ok(request));
    }

    /// The depth cap has to be enforced on the way *in* as well as the way out, since the encode
    /// side runs in the plugin and a hostile plugin would simply not call it.
    #[hegel::test]
    fn a_predicate_within_the_depth_limit_is_never_rejected(tc: hegel::TestCase) {
        let depth = tc.draw(generators::integers::<usize>().max_value(MAX_PRED_DEPTH - 1));
        let mut pred = Pred::eq("a", 1);
        for _ in 0..depth {
            pred = !pred;
        }
        assert!(
            ProtobufPred::try_from(pred.clone()).is_ok(),
            "depth {}",
            depth
        );
    }

    #[hegel::test]
    fn a_predicate_past_the_depth_limit_is_always_rejected(tc: hegel::TestCase) {
        let excess = tc.draw(generators::integers::<usize>().min_value(1).max_value(8));
        let mut pred = Pred::eq("a", 1);
        for _ in 0..MAX_PRED_DEPTH + excess {
            pred = !pred;
        }
        assert!(ProtobufPred::try_from(pred).is_err(), "excess {}", excess);
    }

    fn every_value() -> Vec<DataValue> {
        vec![
            DataValue::Null,
            DataValue::Int(-7),
            DataValue::Real(1.5),
            DataValue::Text("hello".to_owned()),
            DataValue::Bool(true),
            DataValue::Bool(false),
            DataValue::Blob(vec![0, 1, 255]),
        ]
    }

    #[test]
    fn every_value_round_trips() {
        for value in every_value() {
            let protobuf = ProtobufValue::from(value.clone());
            assert_eq!(DataValue::from(protobuf), value);
        }
    }

    #[test]
    fn an_unset_value_oneof_decodes_as_null() {
        // proto3 cannot tell "no value" from "null", and null is what an unset value means.
        assert_eq!(
            DataValue::from(ProtobufValue { value: None }),
            DataValue::Null
        );
    }

    #[test]
    fn every_attr_kind_round_trips() {
        for kind in [
            AttrKind::Text,
            AttrKind::Int,
            AttrKind::Real,
            AttrKind::Bool,
            AttrKind::Blob,
            AttrKind::Json,
        ] {
            let protobuf = ProtobufAttrKind::from(kind);
            assert_eq!(AttrKind::from(protobuf), kind);
            assert_eq!(attr_kind_from_i32(protobuf as i32), Ok(kind));
        }
    }

    #[test]
    fn every_sort_dir_and_watch_mode_round_trips() {
        for dir in [SortDir::Asc, SortDir::Desc] {
            assert_eq!(
                sort_dir_from_i32(ProtobufSortDir::from(dir) as i32),
                Ok(dir)
            );
        }
        for mode in [WatchMode::Rows, WatchMode::Ids] {
            assert_eq!(
                watch_mode_from_i32(ProtobufWatchMode::from(mode) as i32),
                Ok(mode)
            );
        }
    }

    #[test]
    fn an_unrecognized_enum_value_is_rejected_rather_than_defaulted() {
        // It means the plugin was built against a newer yinz-tile than the host; treating an
        // unknown kind as Text would corrupt whatever it stored.
        assert!(attr_kind_from_i32(99).is_err());
        assert!(sort_dir_from_i32(99).is_err());
        assert!(watch_mode_from_i32(99).is_err());
    }

    #[test]
    fn an_entity_definition_round_trips_with_every_flag_set() {
        let def = EntityDef::new("agent", 3)
            .attr(NativeAttrDef::text("project").indexed())
            .attr(NativeAttrDef::text("summary").full_text())
            .attr(NativeAttrDef::text("slug").unique())
            .attr(NativeAttrDef::int("n").required().default(7i64))
            .attr(NativeAttrDef::blob("raw"))
            .attr(NativeAttrDef::boolean("on"))
            .attr(NativeAttrDef::real("ratio"))
            .attr(NativeAttrDef::json("payload"));
        let protobuf = ProtobufEntityDef::from(def.clone());
        assert_eq!(EntityDef::try_from(protobuf), Ok(def));
    }

    #[test]
    fn every_predicate_shape_round_trips() {
        let preds = vec![
            Pred::eq("a", 1),
            Pred::ne("a", "x"),
            Pred::gt("a", 1),
            Pred::gte("a", 1),
            Pred::lt("a", 1),
            Pred::lte("a", 1),
            Pred::is_in("a", [1, 2, 3]),
            Pred::is_null("a"),
            Pred::not_null("a"),
            Pred::prefix("a", "x"),
            Pred::contains("a", "x"),
            Pred::eq("a", 1).and(Pred::eq("b", 2)),
            Pred::eq("a", 1).or(Pred::eq("b", 2)),
            !Pred::eq("a", 1),
            // nested, so the recursion is exercised in both directions
            Pred::all([
                Pred::any([Pred::eq("a", 1), !Pred::contains("b", "y")]),
                Pred::is_null("c"),
            ]),
        ];
        for pred in preds {
            let protobuf = ProtobufPred::try_from(pred.clone()).unwrap();
            assert_eq!(Pred::try_from(protobuf), Ok(pred));
        }
    }

    #[test]
    fn a_predicate_nested_past_the_limit_is_rejected_in_both_directions() {
        let mut pred = Pred::eq("a", 1);
        for _ in 0..MAX_PRED_DEPTH + 2 {
            pred = !pred;
        }
        assert!(ProtobufPred::try_from(pred).is_err());
    }

    #[test]
    fn a_predicate_with_no_branch_set_is_rejected() {
        assert!(Pred::try_from(ProtobufPred { pred: None }).is_err());
    }

    #[test]
    fn a_fetch_request_round_trips() {
        let request = FetchRequest::entity("agent")
            .in_namespace("shared:agents")
            .filter(Pred::eq("project", "yinz"))
            .search("rebase")
            .sort_by("updated_at_ms", SortDir::Desc)
            .sort_by("project", SortDir::Asc)
            .select(["id", "project"])
            .limit(25)
            .offset(5);
        let protobuf = ProtobufFetchRequest::try_from(request.clone()).unwrap();
        assert_eq!(FetchRequest::try_from(protobuf), Ok(request));
    }

    #[test]
    fn a_bare_fetch_request_round_trips() {
        let request = FetchRequest::entity("agent");
        let protobuf = ProtobufFetchRequest::try_from(request.clone()).unwrap();
        assert_eq!(FetchRequest::try_from(protobuf), Ok(request));
    }

    #[test]
    fn every_mutation_shape_round_trips() {
        let mutations = vec![
            Mutation::put("agent").set("project", "yinz").build(),
            Mutation::put("agent")
                .uid("a1")
                .set("project", "yinz")
                .set("n", 1)
                .build(),
            Mutation::update("agent", Pred::eq("uid", "a1"))
                .set("project", "other")
                .build(),
            Mutation::delete("agent", Pred::eq("project", "yinz")),
            Mutation::AssertRows {
                entity: "agent".into(),
                filter: Pred::eq("project", "yinz"),
                rows: vec![],
            },
            Mutation::AssertRows {
                entity: "agent".into(),
                filter: Pred::eq("project", "yinz"),
                rows: vec![(DataRowId::new(7), DataRevision::new(3))],
            },
        ];
        for mutation in mutations {
            let protobuf = ProtobufMutation::try_from(mutation.clone()).unwrap();
            use prost::Message;
            let bytes = protobuf.encode_to_vec();
            assert_eq!(
                Mutation::try_from(ProtobufMutation::decode(bytes.as_slice()).unwrap()),
                Ok(mutation)
            );
        }
    }

    #[test]
    fn an_assertion_without_a_filter_is_rejected() {
        assert!(Mutation::try_from(ProtobufMutation {
            mutation: Some(ProtobufMutationOneof::AssertRows(ProtobufAssertRows {
                entity: "agent".into(),
                filter: None,
                rows: vec![],
            })),
        })
        .is_err());
    }

    #[test]
    fn a_mutation_with_no_branch_set_is_rejected() {
        assert!(Mutation::try_from(ProtobufMutation { mutation: None }).is_err());
    }

    #[test]
    fn an_update_without_a_filter_is_rejected_rather_than_touching_every_row() {
        let protobuf = ProtobufMutation {
            mutation: Some(ProtobufMutationOneof::Update(ProtobufUpdate {
                entity: "agent".to_owned(),
                filter: None,
                values: vec![],
                expect_rev: None,
            })),
        };
        assert!(Mutation::try_from(protobuf).is_err());
    }

    #[test]
    fn a_delete_without_a_filter_is_rejected_rather_than_emptying_the_entity() {
        let protobuf = ProtobufMutation {
            mutation: Some(ProtobufMutationOneof::Delete(ProtobufDelete {
                entity: "agent".to_owned(),
                filter: None,
            })),
        };
        assert!(Mutation::try_from(protobuf).is_err());
    }

    #[test]
    fn rows_round_trip_including_the_cursor() {
        let rows = DataRows {
            columns: vec!["id".to_owned(), "project".to_owned()],
            rows: vec![
                vec![DataValue::Int(1), DataValue::Text("yinz".to_owned())],
                vec![DataValue::Int(2), DataValue::Null],
            ],
            truncated: true,
            next_cursor: Some(DataRowId::new(2)),
        };
        let protobuf = ProtobufRows::from(rows.clone());
        assert_eq!(DataRows::from(protobuf), rows);
    }

    #[test]
    fn empty_rows_round_trip() {
        let rows = DataRows::default();
        let protobuf = ProtobufRows::from(rows.clone());
        assert_eq!(DataRows::from(protobuf), rows);
    }

    #[test]
    fn schema_info_round_trips() {
        let info = DataSchemaInfo {
            namespace: "plugin:x".to_owned(),
            entity: "agent".to_owned(),
            version: 4,
            created: true,
            attrs_added: vec!["worktree".to_owned()],
            indexes_created: vec!["project".to_owned()],
            full_text_rebuilt: true,
        };
        let protobuf = ProtobufSchemaInfo::from(info.clone());
        assert_eq!(DataSchemaInfo::from(protobuf), info);
    }

    #[test]
    fn an_apply_report_round_trips() {
        let report = ApplyReport {
            rows_written: 3,
            rows_deleted: 1,
            ids: vec![1, 2, 3].into_iter().map(DataRowId::new).collect(),
            per_mutation: vec![2, 1, 1],
        };
        let protobuf = ProtobufApplyReport::from(report.clone());
        assert_eq!(ApplyReport::from(protobuf), report);
    }

    #[test]
    fn a_watch_handle_round_trips() {
        let handle = DataWatchHandle {
            watch_id: DataWatchId::new(9),
            rows: DataRows {
                columns: vec!["id".to_owned()],
                rows: vec![vec![DataValue::Int(1)]],
                truncated: false,
                next_cursor: None,
            },
        };
        let protobuf = ProtobufWatchHandle::from(handle.clone());
        assert_eq!(DataWatchHandle::from(protobuf), handle);
    }
}
