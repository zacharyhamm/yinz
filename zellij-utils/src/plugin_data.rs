//! Types for the plugin data store: entity definitions, values, predicates, fetch requests and
//! mutations.
//!
//! Deliberately free of `rusqlite`, and of anything else that cannot build for `wasm32-wasip1`:
//! these types cross the plugin boundary, so they are compiled into every plugin as well as into
//! the host. The engine that turns them into SQL is [`crate::plugin_data_store`], which is
//! host-only for exactly that reason.
//!
//! Name validation lives here rather than in the engine so that a plugin can reject a bad entity
//! or attribute name locally, and get the same message it would have got from the host.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Rows returned by a single fetch when the request doesn't say.
pub const DEFAULT_FETCH_LIMIT: u32 = 100;

/// Hard ceiling on rows per fetch, whatever the request asks for.
///
/// The plugin transport is protobuf wrapped in JSON — payloads cross as a JSON array of decimal
/// numbers, roughly 3-4x the protobuf size — into a guest whose linear memory is capped at 16 MiB
/// and configured to *trap* rather than fail gracefully when it can't grow. Truncating a fetch
/// gives the plugin a `next_cursor` to page with; overrunning kills it.
pub const MAX_FETCH_ROWS: u32 = 1000;

/// Byte ceiling on the encoded rows of a single fetch, applied alongside [`MAX_FETCH_ROWS`].
///
/// A thousand rows of short text is nothing; a thousand rows each holding a 64 KiB blob is far past
/// what the guest can hold. Whichever limit is reached first truncates.
pub const MAX_FETCH_BYTES: usize = 2 * 1024 * 1024;

/// Nesting limit for [`Pred`] trees, enforced on both sides of the boundary.
///
/// Predicates are recursive, so a malicious or buggy plugin could otherwise send a structure deep
/// enough to blow the stack while being converted from protobuf.
pub const MAX_PRED_DEPTH: usize = 16;

/// The longest an entity, namespace segment or attribute name may be.
pub const MAX_NAME_LEN: usize = 63;

/// Prefix that marks a namespace as shared between plugins.
pub const SHARED_NAMESPACE_PREFIX: &str = "shared:";

/// Columns the store owns on every entity table. A declared attribute may not take these names.
pub const RESERVED_ATTR_NAMES: &[&str] = &["id", "uid", "created_at", "updated_at", "rowid", "oid"];

/// The storage class of a declared attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttrKind {
    Text,
    Int,
    Real,
    Bool,
    Blob,
    /// Stored as text, but validated as JSON on write so that it can be relied on later.
    Json,
}

impl AttrKind {
    /// The SQLite column type for this kind.
    pub fn sql_type(&self) -> &'static str {
        match self {
            AttrKind::Text | AttrKind::Json => "TEXT",
            // SQLite has no boolean type; it stores 0/1 in an INTEGER column.
            AttrKind::Int | AttrKind::Bool => "INTEGER",
            AttrKind::Real => "REAL",
            AttrKind::Blob => "BLOB",
        }
    }

    /// Whether an attribute of this kind may be full-text indexed. FTS5 tokenizes text; there is
    /// nothing to tokenize in a number, and indexing a blob would index its bytes as if they were
    /// words.
    pub fn can_full_text(&self) -> bool {
        matches!(self, AttrKind::Text | AttrKind::Json)
    }
}

/// A value in the store. The variants map one-to-one onto SQLite's storage classes, plus `Bool`
/// which is an `Int` with an interpretation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DataValue {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Bool(bool),
    Blob(Vec<u8>),
}

impl DataValue {
    /// Whether this value can be stored in a column of `kind`.
    ///
    /// `Null` is accepted for every kind — whether it is *allowed* is the column's `NOT NULL`
    /// constraint, which SQLite enforces, not a type question.
    pub fn fits(&self, kind: AttrKind) -> bool {
        match (self, kind) {
            (DataValue::Null, _) => true,
            (DataValue::Text(_), AttrKind::Text) | (DataValue::Text(_), AttrKind::Json) => true,
            (DataValue::Int(_), AttrKind::Int) => true,
            (DataValue::Real(_), AttrKind::Real) => true,
            // An integer is a fine `Real` — SQLite would coerce it anyway, and rejecting it would
            // mean a plugin writing `0` to a float column had to know to write `0.0`.
            (DataValue::Int(_), AttrKind::Real) => true,
            (DataValue::Bool(_), AttrKind::Bool) => true,
            (DataValue::Blob(_), AttrKind::Blob) => true,
            _ => false,
        }
    }

    /// A short name for this value's kind, for error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            DataValue::Null => "null",
            DataValue::Int(_) => "int",
            DataValue::Real(_) => "real",
            DataValue::Text(_) => "text",
            DataValue::Bool(_) => "bool",
            DataValue::Blob(_) => "blob",
        }
    }

    /// Roughly how many bytes this value costs on the wire, for the [`MAX_FETCH_BYTES`] budget.
    pub fn encoded_size(&self) -> usize {
        match self {
            DataValue::Null => 1,
            DataValue::Bool(_) => 1,
            DataValue::Int(_) => 8,
            DataValue::Real(_) => 8,
            DataValue::Text(s) => s.len(),
            DataValue::Blob(b) => b.len(),
        }
    }
}

impl From<&str> for DataValue {
    fn from(v: &str) -> Self {
        DataValue::Text(v.to_owned())
    }
}

impl From<String> for DataValue {
    fn from(v: String) -> Self {
        DataValue::Text(v)
    }
}

impl From<i64> for DataValue {
    fn from(v: i64) -> Self {
        DataValue::Int(v)
    }
}

impl From<i32> for DataValue {
    fn from(v: i32) -> Self {
        DataValue::Int(v as i64)
    }
}

impl From<u32> for DataValue {
    fn from(v: u32) -> Self {
        DataValue::Int(v as i64)
    }
}

impl From<usize> for DataValue {
    /// Saturates rather than wrapping. A `usize` past `i64::MAX` is a bug in the caller, and
    /// clamping keeps comparisons ordered where wrapping would flip them negative.
    fn from(v: usize) -> Self {
        DataValue::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}

impl From<u64> for DataValue {
    /// Saturates rather than wrapping, for the reason given on the `usize` conversion.
    fn from(v: u64) -> Self {
        DataValue::Int(i64::try_from(v).unwrap_or(i64::MAX))
    }
}

impl From<f64> for DataValue {
    fn from(v: f64) -> Self {
        DataValue::Real(v)
    }
}

impl From<bool> for DataValue {
    fn from(v: bool) -> Self {
        DataValue::Bool(v)
    }
}

impl From<Vec<u8>> for DataValue {
    fn from(v: Vec<u8>) -> Self {
        DataValue::Blob(v)
    }
}

impl<T: Into<DataValue>> From<Option<T>> for DataValue {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(v) => v.into(),
            None => DataValue::Null,
        }
    }
}

/// One declared attribute of an entity.
///
/// Build with the per-kind constructors and the flag methods:
///
/// ```
/// # use zellij_utils::plugin_data::AttrDef;
/// AttrDef::text("project").indexed();
/// AttrDef::text("summary").full_text();
/// AttrDef::int("updated_at_ms").indexed().required().default(0i64);
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttrDef {
    pub name: String,
    pub kind: AttrKind,
    /// Give this attribute its own btree index.
    pub indexed: bool,
    /// Include this attribute in the entity's full-text index.
    pub full_text: bool,
    /// Enforce uniqueness across rows.
    pub unique: bool,
    /// `NOT NULL`.
    pub required: bool,
    /// Column default. Required when adding a `required` attribute to an entity that already
    /// exists, because SQLite cannot add a `NOT NULL` column without one.
    pub default: Option<DataValue>,
}

impl AttrDef {
    pub fn new(name: impl Into<String>, kind: AttrKind) -> Self {
        AttrDef {
            name: name.into(),
            kind,
            indexed: false,
            full_text: false,
            unique: false,
            required: false,
            default: None,
        }
    }

    pub fn text(name: impl Into<String>) -> Self {
        Self::new(name, AttrKind::Text)
    }

    pub fn int(name: impl Into<String>) -> Self {
        Self::new(name, AttrKind::Int)
    }

    pub fn real(name: impl Into<String>) -> Self {
        Self::new(name, AttrKind::Real)
    }

    pub fn boolean(name: impl Into<String>) -> Self {
        Self::new(name, AttrKind::Bool)
    }

    pub fn blob(name: impl Into<String>) -> Self {
        Self::new(name, AttrKind::Blob)
    }

    pub fn json(name: impl Into<String>) -> Self {
        Self::new(name, AttrKind::Json)
    }

    pub fn indexed(mut self) -> Self {
        self.indexed = true;
        self
    }

    pub fn full_text(mut self) -> Self {
        self.full_text = true;
        self
    }

    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn default(mut self, value: impl Into<DataValue>) -> Self {
        self.default = Some(value.into());
        self
    }
}

/// An entity: a named, typed collection of rows.
///
/// `version` is the plugin's own bookkeeping. The host migrates by diffing the stored definition
/// against the incoming one, not by comparing versions, so bumping it is only required to
/// authorise a destructive change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDef {
    pub name: String,
    pub version: u32,
    pub attrs: Vec<AttrDef>,
}

impl EntityDef {
    pub fn new(name: impl Into<String>, version: u32) -> Self {
        EntityDef {
            name: name.into(),
            version,
            attrs: Vec::new(),
        }
    }

    pub fn attr(mut self, attr: AttrDef) -> Self {
        self.attrs.push(attr);
        self
    }

    pub fn attrs(mut self, attrs: impl IntoIterator<Item = AttrDef>) -> Self {
        self.attrs.extend(attrs);
        self
    }

    pub fn attr_named(&self, name: &str) -> Option<&AttrDef> {
        self.attrs.iter().find(|a| a.name == name)
    }

    /// Every attribute in the full-text index, in declaration order.
    pub fn full_text_attrs(&self) -> Vec<&AttrDef> {
        self.attrs.iter().filter(|a| a.full_text).collect()
    }

    /// Check the definition on its own terms, before any storage is touched.
    pub fn validate(&self) -> Result<(), DataError> {
        validate_name(&self.name, NameKind::Entity)?;
        if self.attrs.is_empty() {
            return Err(DataError::InvalidSchema(format!(
                "entity `{}` declares no attributes",
                self.name
            )));
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.attrs.len());
        for attr in &self.attrs {
            validate_name(&attr.name, NameKind::Attribute)?;
            if seen.contains(&attr.name.as_str()) {
                return Err(DataError::InvalidSchema(format!(
                    "entity `{}` declares attribute `{}` twice",
                    self.name, attr.name
                )));
            }
            seen.push(&attr.name);
            if attr.full_text && !attr.kind.can_full_text() {
                return Err(DataError::InvalidSchema(format!(
                    "attribute `{}` is {:?}, which cannot be full-text indexed; only Text and \
                     Json can",
                    attr.name, attr.kind
                )));
            }
            if let Some(default) = &attr.default {
                if !default.fits(attr.kind) {
                    return Err(DataError::InvalidSchema(format!(
                        "attribute `{}` is {:?} but its default is {}",
                        attr.name,
                        attr.kind,
                        default.kind_name()
                    )));
                }
            }
        }
        Ok(())
    }
}

/// What a name is for, so that validation failures can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    Namespace,
    Entity,
    Attribute,
}

impl fmt::Display for NameKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameKind::Namespace => write!(f, "namespace"),
            NameKind::Entity => write!(f, "entity"),
            NameKind::Attribute => write!(f, "attribute"),
        }
    }
}

/// Check a single name against `^[a-z][a-z0-9_]{0,62}$`, and reject the store's own column names.
///
/// Entity names never reach SQL — the store maps them through a registry table to `e_<rowid>` — but
/// attribute names *are* interpolated as column names, so this is the boundary that keeps a plugin
/// from writing SQL. Applying the same rule to all three keeps one error message and no way to
/// forget which names were checked.
pub fn validate_name(name: &str, kind: NameKind) -> Result<(), DataError> {
    if name.is_empty() {
        return Err(DataError::InvalidName(format!("{} name is empty", kind)));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(DataError::InvalidName(format!(
            "{} name `{}` is {} characters, over the {} limit",
            kind,
            name,
            name.len(),
            MAX_NAME_LEN
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or('\0');
    if !first.is_ascii_lowercase() {
        return Err(DataError::InvalidName(format!(
            "{} name `{}` must start with a lowercase ascii letter",
            kind, name
        )));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(DataError::InvalidName(format!(
                "{} name `{}` may only contain lowercase ascii letters, digits and underscores",
                kind, name
            )));
        }
    }
    if kind == NameKind::Attribute && RESERVED_ATTR_NAMES.contains(&name) {
        return Err(DataError::InvalidName(format!(
            "`{}` is reserved by the store and cannot be an attribute name",
            name
        )));
    }
    Ok(())
}

/// Check a name used to *refer* to a column, rather than to declare one.
///
/// `select` and `sort` may both legitimately name the store's own columns — sorting by
/// `updated_at` is the commonest query there is, and `created_at`/`updated_at` are indexed for
/// exactly that. So the reserved-name rule, which exists to stop a declaration colliding with those
/// columns, must not apply here.
pub fn validate_column_reference(name: &str) -> Result<(), DataError> {
    if RESERVED_ATTR_NAMES.contains(&name) {
        return Ok(());
    }
    validate_name(name, NameKind::Attribute)
}

/// Check a namespace as given by a plugin.
///
/// `None` means the plugin's own private namespace, which the host derives — a plugin cannot name
/// it, and so cannot name anyone else's either. `Some` must be `shared:`-prefixed, and the segment
/// after the prefix is validated like any other name.
pub fn validate_namespace(namespace: Option<&str>) -> Result<(), DataError> {
    let Some(namespace) = namespace else {
        return Ok(());
    };
    let Some(segment) = namespace.strip_prefix(SHARED_NAMESPACE_PREFIX) else {
        return Err(DataError::InvalidName(format!(
            "namespace `{}` must start with `{}`; a plugin's own namespace is addressed by \
             leaving the namespace unset",
            namespace, SHARED_NAMESPACE_PREFIX
        )));
    };
    validate_name(segment, NameKind::Namespace)
}

/// Sort direction for a fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    pub fn sql(&self) -> &'static str {
        match self {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        }
    }
}

/// A filter over rows.
///
/// Combine with [`Pred::and`], [`Pred::or`] and `!`. Every leaf names an attribute; the
/// host checks that the attribute is declared before compiling anything, so a typo is an error
/// rather than an empty result set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Pred {
    Eq(String, DataValue),
    Ne(String, DataValue),
    Gt(String, DataValue),
    Gte(String, DataValue),
    Lt(String, DataValue),
    Lte(String, DataValue),
    In(String, Vec<DataValue>),
    IsNull(String),
    NotNull(String),
    /// `LIKE 'value%'`. Can use an index.
    Prefix(String, String),
    /// `LIKE '%value%'`. Cannot use an index at any size — declare the attribute `full_text` and
    /// use [`FetchRequest::search`] instead if this is a hot path.
    Contains(String, String),
    And(Vec<Pred>),
    Or(Vec<Pred>),
    Not(Box<Pred>),
}

impl Pred {
    pub fn eq(attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        Pred::Eq(attr.into(), value.into())
    }

    pub fn ne(attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        Pred::Ne(attr.into(), value.into())
    }

    pub fn gt(attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        Pred::Gt(attr.into(), value.into())
    }

    pub fn gte(attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        Pred::Gte(attr.into(), value.into())
    }

    pub fn lt(attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        Pred::Lt(attr.into(), value.into())
    }

    pub fn lte(attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        Pred::Lte(attr.into(), value.into())
    }

    pub fn is_in(
        attr: impl Into<String>,
        values: impl IntoIterator<Item = impl Into<DataValue>>,
    ) -> Self {
        Pred::In(attr.into(), values.into_iter().map(Into::into).collect())
    }

    pub fn is_null(attr: impl Into<String>) -> Self {
        Pred::IsNull(attr.into())
    }

    pub fn not_null(attr: impl Into<String>) -> Self {
        Pred::NotNull(attr.into())
    }

    pub fn prefix(attr: impl Into<String>, value: impl Into<String>) -> Self {
        Pred::Prefix(attr.into(), value.into())
    }

    pub fn contains(attr: impl Into<String>, value: impl Into<String>) -> Self {
        Pred::Contains(attr.into(), value.into())
    }

    /// Combine with another predicate. Flattens nested `And`s so that a chain of `.and()` calls
    /// stays one level deep rather than nesting toward [`MAX_PRED_DEPTH`].
    pub fn and(self, other: Pred) -> Self {
        match self {
            Pred::And(mut preds) => {
                preds.push(other);
                Pred::And(preds)
            },
            first => Pred::And(vec![first, other]),
        }
    }

    /// Combine with another predicate, flattening nested `Or`s as [`Pred::and`] does.
    pub fn or(self, other: Pred) -> Self {
        match self {
            Pred::Or(mut preds) => {
                preds.push(other);
                Pred::Or(preds)
            },
            first => Pred::Or(vec![first, other]),
        }
    }

    pub fn all(preds: impl IntoIterator<Item = Pred>) -> Self {
        Pred::And(preds.into_iter().collect())
    }

    pub fn any(preds: impl IntoIterator<Item = Pred>) -> Self {
        Pred::Or(preds.into_iter().collect())
    }

    /// How deeply this predicate nests. A leaf is 1.
    pub fn depth(&self) -> usize {
        match self {
            Pred::And(preds) | Pred::Or(preds) => {
                1 + preds.iter().map(Pred::depth).max().unwrap_or(0)
            },
            Pred::Not(pred) => 1 + pred.depth(),
            _ => 1,
        }
    }

    /// Every attribute name mentioned anywhere in the tree.
    pub fn referenced_attrs(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_attrs(&mut out);
        out
    }

    fn collect_attrs<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Pred::Eq(a, _)
            | Pred::Ne(a, _)
            | Pred::Gt(a, _)
            | Pred::Gte(a, _)
            | Pred::Lt(a, _)
            | Pred::Lte(a, _)
            | Pred::In(a, _)
            | Pred::IsNull(a)
            | Pred::NotNull(a)
            | Pred::Prefix(a, _)
            | Pred::Contains(a, _) => out.push(a.as_str()),
            Pred::And(preds) | Pred::Or(preds) => {
                for pred in preds {
                    pred.collect_attrs(out);
                }
            },
            Pred::Not(pred) => pred.collect_attrs(out),
        }
    }

    /// Reject a tree deeper than [`MAX_PRED_DEPTH`], and one whose `And`/`Or` have no branches.
    pub fn validate(&self) -> Result<(), DataError> {
        if self.depth() > MAX_PRED_DEPTH {
            return Err(DataError::InvalidQuery(format!(
                "predicate nests {} levels deep, over the {} limit",
                self.depth(),
                MAX_PRED_DEPTH
            )));
        }
        self.validate_shape()
    }

    fn validate_shape(&self) -> Result<(), DataError> {
        match self {
            Pred::And(preds) | Pred::Or(preds) => {
                if preds.is_empty() {
                    return Err(DataError::InvalidQuery(
                        "an And/Or predicate has no branches".to_owned(),
                    ));
                }
                for pred in preds {
                    pred.validate_shape()?;
                }
                Ok(())
            },
            Pred::Not(pred) => pred.validate_shape(),
            Pred::In(attr, values) => {
                if values.is_empty() {
                    return Err(DataError::InvalidQuery(format!(
                        "`{}` is matched against an empty In list, which can never be true; use \
                         a filter that says so",
                        attr
                    )));
                }
                Ok(())
            },
            _ => Ok(()),
        }
    }
}

impl std::ops::Not for Pred {
    type Output = Pred;

    fn not(self) -> Pred {
        Pred::Not(Box::new(self))
    }
}

/// A read against one entity.
///
/// `limit` is clamped to [`MAX_FETCH_ROWS`] and rows are additionally cut off at
/// [`MAX_FETCH_BYTES`]; either truncation sets [`DataRows::truncated`] and supplies a
/// `next_cursor` to pass back as `after_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FetchRequest {
    /// `None` for the calling plugin's own namespace.
    pub namespace: Option<String>,
    pub entity: String,
    pub filter: Option<Pred>,
    /// FTS5 query over the entity's full-text attributes. A string that isn't valid FTS5 syntax is
    /// retried as a literal phrase rather than failing, so anything pasted in will search.
    pub search: Option<String>,
    /// Empty sorts by `id` ascending, or by relevance when `search` is set.
    pub sort: Vec<(String, SortDir)>,
    /// Empty selects `id`, `uid`, `created_at`, `updated_at` and every declared attribute.
    pub select: Vec<String>,
    pub limit: u32,
    pub offset: u32,
    /// Keyset pagination: return only rows with `id` greater than this. Prefer this to `offset`
    /// for walking a large set, since it doesn't re-scan what it has already returned.
    pub after_id: Option<i64>,
}

impl FetchRequest {
    pub fn entity(entity: impl Into<String>) -> Self {
        FetchRequest {
            namespace: None,
            entity: entity.into(),
            filter: None,
            search: None,
            sort: Vec::new(),
            select: Vec::new(),
            limit: DEFAULT_FETCH_LIMIT,
            offset: 0,
            after_id: None,
        }
    }

    /// Read from a `shared:` namespace instead of the plugin's own.
    pub fn in_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.namespace = Some(namespace.into());
        self
    }

    pub fn filter(mut self, pred: Pred) -> Self {
        self.filter = Some(match self.filter {
            Some(existing) => existing.and(pred),
            None => pred,
        });
        self
    }

    pub fn search(mut self, query: impl Into<String>) -> Self {
        self.search = Some(query.into());
        self
    }

    pub fn sort_by(mut self, attr: impl Into<String>, dir: SortDir) -> Self {
        self.sort.push((attr.into(), dir));
        self
    }

    pub fn select(mut self, attrs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.select.extend(attrs.into_iter().map(Into::into));
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = limit;
        self
    }

    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = offset;
        self
    }

    pub fn after_id(mut self, id: i64) -> Self {
        self.after_id = Some(id);
        self
    }

    /// The limit actually applied, after clamping.
    pub fn effective_limit(&self) -> u32 {
        if self.limit == 0 {
            DEFAULT_FETCH_LIMIT
        } else {
            self.limit.min(MAX_FETCH_ROWS)
        }
    }

    /// Check everything about the request that doesn't need the schema.
    pub fn validate(&self) -> Result<(), DataError> {
        validate_namespace(self.namespace.as_deref())?;
        validate_name(&self.entity, NameKind::Entity)?;
        if let Some(filter) = &self.filter {
            filter.validate()?;
        }
        for (attr, _) in &self.sort {
            validate_column_reference(attr)?;
        }
        for attr in &self.select {
            validate_column_reference(attr)?;
        }
        Ok(())
    }
}

/// A write. A batch of these is applied in one transaction, all or nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mutation {
    /// Insert a row, or replace the one with this `uid` if it exists. With `uid` unset it always
    /// inserts.
    Put {
        entity: String,
        uid: Option<String>,
        values: Vec<(String, DataValue)>,
    },
    /// Set attributes on every row matching `filter`.
    Update {
        entity: String,
        filter: Pred,
        values: Vec<(String, DataValue)>,
    },
    /// Remove every row matching `filter`.
    Delete { entity: String, filter: Pred },
}

impl Mutation {
    pub fn put(entity: impl Into<String>) -> PutBuilder {
        PutBuilder {
            entity: entity.into(),
            uid: None,
            values: Vec::new(),
        }
    }

    pub fn update(entity: impl Into<String>, filter: Pred) -> UpdateBuilder {
        UpdateBuilder {
            entity: entity.into(),
            filter,
            values: Vec::new(),
        }
    }

    pub fn delete(entity: impl Into<String>, filter: Pred) -> Self {
        Mutation::Delete {
            entity: entity.into(),
            filter,
        }
    }

    /// The entity this mutation touches, for matching against watches.
    pub fn entity(&self) -> &str {
        match self {
            Mutation::Put { entity, .. }
            | Mutation::Update { entity, .. }
            | Mutation::Delete { entity, .. } => entity,
        }
    }

    pub fn validate(&self) -> Result<(), DataError> {
        validate_name(self.entity(), NameKind::Entity)?;
        match self {
            Mutation::Put { entity, values, .. } => {
                if values.is_empty() {
                    return Err(DataError::InvalidQuery(format!(
                        "a Put into `{}` sets no attributes",
                        entity
                    )));
                }
                validate_value_attrs(values)
            },
            Mutation::Update {
                entity,
                filter,
                values,
            } => {
                filter.validate()?;
                if values.is_empty() {
                    return Err(DataError::InvalidQuery(format!(
                        "an Update of `{}` sets no attributes",
                        entity
                    )));
                }
                validate_value_attrs(values)
            },
            Mutation::Delete { filter, .. } => filter.validate(),
        }
    }
}

fn validate_value_attrs(values: &[(String, DataValue)]) -> Result<(), DataError> {
    let mut seen: Vec<&str> = Vec::with_capacity(values.len());
    for (attr, _) in values {
        validate_name(attr, NameKind::Attribute)?;
        if seen.contains(&attr.as_str()) {
            return Err(DataError::InvalidQuery(format!(
                "attribute `{}` is set twice in one mutation",
                attr
            )));
        }
        seen.push(attr);
    }
    Ok(())
}

/// Builder for [`Mutation::Put`].
#[derive(Debug, Clone)]
pub struct PutBuilder {
    entity: String,
    uid: Option<String>,
    values: Vec<(String, DataValue)>,
}

impl PutBuilder {
    /// The stable key this row upserts on.
    pub fn uid(mut self, uid: impl Into<String>) -> Self {
        self.uid = Some(uid.into());
        self
    }

    pub fn set(mut self, attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        self.values.push((attr.into(), value.into()));
        self
    }

    pub fn build(self) -> Mutation {
        Mutation::Put {
            entity: self.entity,
            uid: self.uid,
            values: self.values,
        }
    }
}

impl From<PutBuilder> for Mutation {
    fn from(builder: PutBuilder) -> Self {
        builder.build()
    }
}

/// Builder for [`Mutation::Update`].
#[derive(Debug, Clone)]
pub struct UpdateBuilder {
    entity: String,
    filter: Pred,
    values: Vec<(String, DataValue)>,
}

impl UpdateBuilder {
    pub fn set(mut self, attr: impl Into<String>, value: impl Into<DataValue>) -> Self {
        self.values.push((attr.into(), value.into()));
        self
    }

    pub fn build(self) -> Mutation {
        Mutation::Update {
            entity: self.entity,
            filter: self.filter,
            values: self.values,
        }
    }
}

impl From<UpdateBuilder> for Mutation {
    fn from(builder: UpdateBuilder) -> Self {
        builder.build()
    }
}

/// The result of a fetch.
///
/// `columns` is sent once for the whole result set rather than per row: the transport amplifies
/// every byte by 3-4x, and a column name repeated per row is the single easiest thing to not send.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DataRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<DataValue>>,
    /// Set when the fetch stopped at a row or byte limit rather than at the end of the results.
    pub truncated: bool,
    /// The `id` of the last row returned, to pass back as [`FetchRequest::after_id`].
    pub next_cursor: Option<i64>,
}

impl DataRows {
    /// The index of a named column, for reading a row by name.
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| c == name)
    }

    /// Read one value by column name.
    pub fn get<'a>(&'a self, row: &'a [DataValue], column: &str) -> Option<&'a DataValue> {
        self.column_index(column).and_then(|i| row.get(i))
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }
}

/// What a batch of mutations did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ApplyReport {
    pub rows_written: u64,
    pub rows_deleted: u64,
    /// The `id` of each row inserted or replaced by a `Put`, in order.
    pub ids: Vec<i64>,
}

/// What the store did in response to a schema declaration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DataSchemaInfo {
    /// The namespace the entity ended up in — the resolved private one when the request left it
    /// unset, so a plugin can log where its data actually went.
    pub namespace: String,
    pub entity: String,
    /// The version now recorded for this entity.
    pub version: u32,
    /// True when the entity did not exist before this call.
    pub created: bool,
    /// Attributes added to an existing entity by this call.
    pub attrs_added: Vec<String>,
    /// Indexes created by this call, by attribute name.
    pub indexes_created: Vec<String>,
    /// True when the full-text index was built or rebuilt by this call.
    pub full_text_rebuilt: bool,
}

/// How much a watch wants to be told when its results change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WatchMode {
    /// Send the rows themselves, under the same limits as a fetch.
    #[default]
    Rows,
    /// Send only the `id` column. For a watcher over a large set that would rather page for the
    /// rows it actually needs than receive all of them on every change.
    Ids,
}

/// A registered watch, and the results at the moment it was registered.
///
/// The initial rows come back with the handle so that a plugin doesn't have to fetch and watch
/// separately, and can't miss a change in the gap between the two.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DataWatchHandle {
    pub watch_id: u64,
    pub rows: DataRows,
}

/// What can go wrong in the store.
///
/// Carries prose rather than codes: these cross the plugin boundary as a string in an
/// `Err` variant, and the message is the only thing the plugin author sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataError {
    /// A namespace, entity or attribute name that isn't allowed.
    InvalidName(String),
    /// An entity definition that doesn't hold together.
    InvalidSchema(String),
    /// A fetch or mutation that doesn't hold together, or that names something undeclared.
    InvalidQuery(String),
    /// A migration the store won't perform without being told to.
    RefusedMigration(String),
    /// The entity hasn't been declared in this namespace.
    UnknownEntity(String),
    /// The plugin lacks the permission for what it asked.
    PermissionDenied(String),
    /// Anything from SQLite or the filesystem.
    Storage(String),
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataError::InvalidName(m)
            | DataError::InvalidSchema(m)
            | DataError::InvalidQuery(m)
            | DataError::RefusedMigration(m)
            | DataError::UnknownEntity(m)
            | DataError::PermissionDenied(m)
            | DataError::Storage(m) => write!(f, "{}", m),
        }
    }
}

impl std::error::Error for DataError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_entity_validates() {
        let def = EntityDef::new("agent", 1)
            .attr(AttrDef::text("project").indexed())
            .attr(AttrDef::text("summary").full_text())
            .attr(AttrDef::int("updated_at_ms").indexed());
        assert_eq!(def.validate(), Ok(()));
    }

    #[test]
    fn an_entity_with_no_attributes_is_rejected() {
        let def = EntityDef::new("agent", 1);
        assert!(matches!(def.validate(), Err(DataError::InvalidSchema(_))));
    }

    #[test]
    fn a_duplicate_attribute_is_rejected() {
        let def = EntityDef::new("agent", 1)
            .attr(AttrDef::text("project"))
            .attr(AttrDef::int("project"));
        assert!(matches!(def.validate(), Err(DataError::InvalidSchema(_))));
    }

    #[test]
    fn only_text_and_json_can_be_full_text_indexed() {
        for kind in [
            AttrKind::Int,
            AttrKind::Real,
            AttrKind::Bool,
            AttrKind::Blob,
        ] {
            let def = EntityDef::new("agent", 1).attr(AttrDef::new("x", kind).full_text());
            assert!(
                matches!(def.validate(), Err(DataError::InvalidSchema(_))),
                "{:?} should not be full-text indexable",
                kind
            );
        }
        for kind in [AttrKind::Text, AttrKind::Json] {
            let def = EntityDef::new("agent", 1).attr(AttrDef::new("x", kind).full_text());
            assert_eq!(def.validate(), Ok(()), "{:?} should be", kind);
        }
    }

    #[test]
    fn a_default_of_the_wrong_kind_is_rejected() {
        let def = EntityDef::new("agent", 1).attr(AttrDef::int("count").default("not a number"));
        assert!(matches!(def.validate(), Err(DataError::InvalidSchema(_))));
    }

    #[test]
    fn reserved_column_names_cannot_be_attributes() {
        for name in RESERVED_ATTR_NAMES {
            assert!(
                matches!(
                    validate_name(name, NameKind::Attribute),
                    Err(DataError::InvalidName(_))
                ),
                "`{}` should be reserved",
                name
            );
        }
    }

    #[test]
    fn reserved_names_are_fine_for_entities() {
        // The reservation is about column names. An entity called `id` never reaches SQL, because
        // entity names are mapped through the registry table.
        assert_eq!(validate_name("id", NameKind::Entity), Ok(()));
    }

    #[test]
    fn names_that_could_carry_sql_are_rejected() {
        for name in [
            "Agent",
            "1agent",
            "_agent",
            "agent-1",
            "agent name",
            "agent\"",
            "agent;drop table x",
            "agent`",
            "",
        ] {
            assert!(
                matches!(
                    validate_name(name, NameKind::Attribute),
                    Err(DataError::InvalidName(_))
                ),
                "`{}` should be rejected",
                name
            );
        }
    }

    #[test]
    fn a_name_at_the_length_limit_is_allowed_and_one_past_it_is_not() {
        let at_limit: String = std::iter::repeat_n('a', MAX_NAME_LEN).collect();
        assert_eq!(validate_name(&at_limit, NameKind::Attribute), Ok(()));
        let past_limit: String = std::iter::repeat_n('a', MAX_NAME_LEN + 1).collect();
        assert!(matches!(
            validate_name(&past_limit, NameKind::Attribute),
            Err(DataError::InvalidName(_))
        ));
    }

    #[test]
    fn an_unset_namespace_is_the_plugins_own() {
        assert_eq!(validate_namespace(None), Ok(()));
    }

    #[test]
    fn a_namespace_must_be_shared_prefixed() {
        assert_eq!(validate_namespace(Some("shared:agents")), Ok(()));
        // Without the rule, this would be a plugin naming another plugin's private namespace.
        for namespace in [
            "agents",
            "plugin:hocus",
            "shared",
            "shared:",
            "shared:Agents",
        ] {
            assert!(
                matches!(
                    validate_namespace(Some(namespace)),
                    Err(DataError::InvalidName(_))
                ),
                "`{}` should be rejected",
                namespace
            );
        }
    }

    #[test]
    fn chained_ands_stay_flat_rather_than_nesting() {
        let pred = Pred::eq("a", 1)
            .and(Pred::eq("b", 2))
            .and(Pred::eq("c", 3))
            .and(Pred::eq("d", 4));
        // Flattening is what keeps a long chain of `.and()` from walking into MAX_PRED_DEPTH.
        assert_eq!(pred.depth(), 2);
        match pred {
            Pred::And(preds) => assert_eq!(preds.len(), 4),
            other => panic!("expected a flat And, got {:?}", other),
        }
    }

    #[test]
    fn chained_ors_stay_flat_too() {
        let pred = Pred::eq("a", 1).or(Pred::eq("b", 2)).or(Pred::eq("c", 3));
        assert_eq!(pred.depth(), 2);
        match pred {
            Pred::Or(preds) => assert_eq!(preds.len(), 3),
            other => panic!("expected a flat Or, got {:?}", other),
        }
    }

    #[test]
    fn a_predicate_deeper_than_the_limit_is_rejected() {
        let mut pred = Pred::eq("a", 1);
        for _ in 0..MAX_PRED_DEPTH {
            pred = !pred;
        }
        assert!(matches!(pred.validate(), Err(DataError::InvalidQuery(_))));
    }

    #[test]
    fn a_predicate_at_the_depth_limit_is_allowed() {
        let mut pred = Pred::eq("a", 1);
        for _ in 0..MAX_PRED_DEPTH - 1 {
            pred = !pred;
        }
        assert_eq!(pred.depth(), MAX_PRED_DEPTH);
        assert_eq!(pred.validate(), Ok(()));
    }

    #[test]
    fn an_empty_in_list_is_rejected_rather_than_matching_nothing() {
        let pred = Pred::In("project".to_owned(), vec![]);
        assert!(matches!(pred.validate(), Err(DataError::InvalidQuery(_))));
    }

    #[test]
    fn an_empty_and_is_rejected() {
        assert!(matches!(
            Pred::And(vec![]).validate(),
            Err(DataError::InvalidQuery(_))
        ));
    }

    #[test]
    fn referenced_attrs_finds_every_leaf() {
        let pred = Pred::eq("a", 1)
            .and(Pred::any([Pred::gt("b", 2), Pred::is_null("c")]))
            .and(!Pred::contains("d", "x"));
        let mut attrs = pred.referenced_attrs();
        attrs.sort_unstable();
        assert_eq!(attrs, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn a_fetch_limit_is_clamped_to_the_maximum() {
        assert_eq!(
            FetchRequest::entity("agent")
                .limit(u32::MAX)
                .effective_limit(),
            MAX_FETCH_ROWS
        );
    }

    #[test]
    fn a_zero_limit_means_the_default_rather_than_no_rows() {
        assert_eq!(
            FetchRequest::entity("agent").limit(0).effective_limit(),
            DEFAULT_FETCH_LIMIT
        );
    }

    #[test]
    fn repeated_filter_calls_and_together() {
        let request = FetchRequest::entity("agent")
            .filter(Pred::eq("project", "zellij"))
            .filter(Pred::gt("updated_at_ms", 5));
        match request.filter {
            Some(Pred::And(preds)) => assert_eq!(preds.len(), 2),
            other => panic!("expected an And of both filters, got {:?}", other),
        }
    }

    #[test]
    fn select_may_name_the_stores_own_columns() {
        let request = FetchRequest::entity("agent").select(["id", "updated_at", "project"]);
        assert_eq!(request.validate(), Ok(()));
    }

    #[test]
    fn a_mutation_that_sets_nothing_is_rejected() {
        assert!(matches!(
            Mutation::put("agent").build().validate(),
            Err(DataError::InvalidQuery(_))
        ));
    }

    #[test]
    fn setting_one_attribute_twice_is_rejected() {
        let mutation = Mutation::put("agent")
            .set("project", "a")
            .set("project", "b")
            .build();
        assert!(matches!(
            mutation.validate(),
            Err(DataError::InvalidQuery(_))
        ));
    }

    #[test]
    fn an_int_is_accepted_for_a_real_column() {
        // SQLite would coerce it anyway, and rejecting it would mean writing `0` to a float
        // column was an error the plugin author had to learn about.
        assert!(DataValue::Int(3).fits(AttrKind::Real));
        assert!(!DataValue::Real(3.0).fits(AttrKind::Int));
    }

    #[test]
    fn null_fits_every_kind_because_nullability_is_the_columns_business() {
        for kind in [
            AttrKind::Text,
            AttrKind::Int,
            AttrKind::Real,
            AttrKind::Bool,
            AttrKind::Blob,
            AttrKind::Json,
        ] {
            assert!(DataValue::Null.fits(kind), "{:?}", kind);
        }
    }

    #[test]
    fn an_option_converts_to_null_when_it_is_none() {
        assert_eq!(DataValue::from(None::<i64>), DataValue::Null);
        assert_eq!(DataValue::from(Some(7i64)), DataValue::Int(7));
    }

    #[test]
    fn oversized_unsigned_values_saturate_rather_than_wrapping_negative() {
        // Wrapping would flip an ordering comparison, which is worse than being wrong at a bound
        // no real timestamp reaches.
        assert_eq!(DataValue::from(u64::MAX), DataValue::Int(i64::MAX));
    }
}
