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

/// NUL is refused at the boundary wherever text can be supplied.
#[test]
fn a_nul_byte_in_text_is_rejected_wherever_text_is_supplied() {
    let put = Mutation::put("agent").set("project", "a\0b").build();
    assert!(matches!(put.validate(), Err(DataError::InvalidQuery(_))));
    let put_uid = Mutation::put("agent")
        .uid("a\0b")
        .set("project", "ok")
        .build();
    assert!(matches!(
        put_uid.validate(),
        Err(DataError::InvalidQuery(_))
    ));
    let filtered = FetchRequest::entity("agent").filter(Pred::eq("project", "a\0b"));
    assert!(matches!(
        filtered.validate(),
        Err(DataError::InvalidQuery(_))
    ));
    let contained = FetchRequest::entity("agent").filter(Pred::contains("project", "\0"));
    assert!(matches!(
        contained.validate(),
        Err(DataError::InvalidQuery(_))
    ));
    let listed = FetchRequest::entity("agent").filter(Pred::is_in("project", ["ok", "\0"]));
    assert!(matches!(listed.validate(), Err(DataError::InvalidQuery(_))));
    let defaulted = EntityDef::new("agent", 1).attr(AttrDef::text("project").default("\0"));
    assert!(matches!(
        defaulted.validate(),
        Err(DataError::InvalidSchema(_))
    ));
    // A blob is the place for bytes, and takes them.
    let blob = Mutation::put("agent").set("raw", vec![0u8, 1]).build();
    assert_eq!(blob.validate(), Ok(()));
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
fn reserved_names_are_fine_for_entities_and_namespaces() {
    // The reservation is about column names. An entity called `id` never reaches SQL, because
    // entity names are mapped through the registry table.
    for name in RESERVED_ATTR_NAMES {
        assert_eq!(validate_name(name, NameKind::Entity), Ok(()));
        assert_eq!(validate_name(name, NameKind::Namespace), Ok(()));
        assert_eq!(validate_namespace(Some(&format!("shared:{name}"))), Ok(()));
    }
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
        "plugin:agent",
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
        .filter(Pred::eq("project", "yinz"))
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
#[test]
fn assert_rows_validates_and_round_trips_json() {
    let assertion = |rows| Mutation::AssertRows {
        entity: "todo".into(),
        filter: Pred::eq("root", "/work"),
        rows,
    };
    for rows in [vec![], vec![(DataRowId::new(1), DataRevision::new(2))]] {
        let mutation = assertion(rows);
        mutation.validate().unwrap();
        let json = serde_json::to_string(&mutation).unwrap();
        assert_eq!(serde_json::from_str::<Mutation>(&json).unwrap(), mutation);
    }
    for rows in [
        vec![(DataRowId::new(0), DataRevision::new(1))],
        vec![(DataRowId::new(1), DataRevision::new(0))],
        vec![(DataRowId::new(1), DataRevision::new(-1))],
        vec![(DataRowId::new(1), DataRevision::new(1)); 2],
    ] {
        assert!(matches!(
            assertion(rows).validate(),
            Err(DataError::InvalidQuery(_))
        ));
    }
}
