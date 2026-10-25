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
    let values =
        generators::vecs(generators::tuples!(generators::text(), any_data_value(),)).max_size(4);
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
