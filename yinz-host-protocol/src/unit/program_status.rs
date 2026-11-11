use super::*;
use base64::engine::general_purpose::STANDARD;

fn b64(text: &str) -> String {
    STANDARD.encode(text)
}

fn parse_body(body: &str) -> Option<ProgramStatusMessage> {
    let mut params: Vec<&[u8]> = vec![b"7501"];
    params.extend(body.split(';').map(|p| p.as_bytes()));
    parse(&params, false)
}

fn set(body: &str) -> ProgramStatusRecord {
    match parse_body(body) {
        Some(ProgramStatusMessage::Set(record)) => record,
        other => panic!("expected a record, got {:?}", other),
    }
}

#[test]
fn query_is_recognized() {
    assert_eq!(parse_body("?"), Some(ProgramStatusMessage::Query));
    assert_eq!(
        query_reply(false),
        b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\"
            .to_vec()
    );
}

#[test]
fn full_report_is_parsed() {
    let record = set(&format!(
        "state=blocked:kind=permission:app=terraform:progress=40:msg={}:title={}",
        b64("Apply 3 to add?"),
        b64("Plan")
    ));
    assert_eq!(record.state, Some(ProgramState::Blocked));
    assert_eq!(record.kind, Some(BlockedKind::Permission));
    assert_eq!(record.app.as_deref(), Some("terraform"));
    assert_eq!(record.progress, Some(40));
    assert_eq!(record.msg.as_deref(), Some("Apply 3 to add?"));
    assert_eq!(record.title.as_deref(), Some("Plan"));
    assert_eq!(record.id, None);
}

#[test]
fn whitespace_is_trimmed_and_last_key_wins() {
    let record = set(" state = working : app=a : app = b ");
    assert_eq!(record.state, Some(ProgramState::Working));
    assert_eq!(record.app.as_deref(), Some("b"));
}

#[test]
fn malformed_pairs_are_skipped() {
    let record = set("state=done:nonsense:=x:APP=y:app=ok:bad=a b");
    assert_eq!(record.state, Some(ProgramState::Done));
    assert_eq!(record.app.as_deref(), Some("ok"));
}

#[test]
fn unknown_keys_are_ignored() {
    let record = set("state=idle:future=1");
    assert_eq!(record.state, Some(ProgramState::Idle));
}

#[test]
fn missing_or_unknown_state_is_ignored() {
    assert_eq!(parse_body("app=x"), None);
    assert_eq!(parse_body("state=sleeping"), None);
}

#[test]
fn kind_and_progress_only_apply_to_their_states() {
    let record = set("state=working:kind=auth:progress=101");
    assert_eq!(record.kind, None);
    assert_eq!(record.progress, None);
    let record = set("state=done:progress=50");
    assert_eq!(record.progress, None);
    let record = set("state=blocked:kind=unknown:progress=7");
    assert_eq!(record.kind, None);
    assert_eq!(record.progress, Some(7));
    let record = set("state=working:progress=-1");
    assert_eq!(record.progress, None);
}

#[test]
fn app_outside_charset_is_absent_and_too_long_discards() {
    assert_eq!(set("state=idle:app=a,b").app, None);
    assert_eq!(
        parse_body(&format!("state=idle:app={}", "a".repeat(33))),
        None
    );
}

#[test]
fn invalid_ids_ignore_the_report() {
    assert_eq!(parse_body("state=idle:id=a//b"), None);
    assert_eq!(parse_body("state=idle:id=a,b"), None);
    assert_eq!(
        parse_body(&format!("state=idle:id={}", "a".repeat(33))),
        None
    );
    assert_eq!(parse_body("state=idle:id=1/2/3/4/5/6/7/8/9"), None);
    let long_id = ["abcdefghijklmno"; 8].join("/");
    assert_eq!(long_id.len(), 127);
    assert!(parse_body(&format!("state=idle:id={}", long_id)).is_some());
    let too_long = ["abcdefghijklmnop"; 8].join("/");
    assert_eq!(parse_body(&format!("state=idle:id={}", too_long)), None);
    assert_eq!(
        set("state=idle:id=build/test").id.as_deref(),
        Some("build/test")
    );
}

#[test]
fn control_characters_in_text_discard_the_report() {
    assert_eq!(parse_body(&format!("state=idle:msg={}", b64("a\nb"))), None);
    assert_eq!(
        parse_body(&format!("state=idle:msg={}", b64("a\u{85}b"))),
        None
    );
    assert_eq!(
        parse_body(&format!("state=idle:title={}", b64("\u{7f}"))),
        None
    );
    assert_eq!(set("state=idle:msg=!!!").msg, None);
    assert_eq!(parse_body("state=idle:msg=_-_-"), None);
}

#[test]
fn padding_is_optional() {
    let record = set("state=idle:msg=YQ");
    assert_eq!(record.msg.as_deref(), Some("a"));
}

#[test]
fn limits_discard_the_report() {
    let long_key = format!("state=idle:{}=1", "a".repeat(17));
    assert_eq!(parse_body(&long_key), None);
    let msg = b64(&"x".repeat(2049));
    assert_eq!(parse_body(&format!("state=idle:msg={}", msg)), None);
    let msg = b64(&"x".repeat(2048));
    assert!(parse_body(&format!("state=idle:msg={}", msg)).is_some());
    let title = b64(&"x".repeat(193));
    assert_eq!(parse_body(&format!("state=idle:title={}", title)), None);
    let huge = format!("state=idle:pad={}", "a".repeat(4100));
    assert_eq!(parse_body(&huge), None);
    let encoded_too_long = format!("state=idle:msg={}", "QUFB".repeat(684));
    assert_eq!(parse_body(&encoded_too_long), None);
}

#[test]
fn clear_reports() {
    assert_eq!(
        parse_body("state=clear"),
        Some(ProgramStatusMessage::Clear { id: None })
    );
    assert_eq!(
        parse_body("state=clear:id=a/b"),
        Some(ProgramStatusMessage::Clear {
            id: Some("a/b".to_owned())
        })
    );
    assert_eq!(parse_body(&format!("state=clear:msg={}", b64("\n"))), None);
}

#[test]
fn reports_replace_records_completely() {
    let mut records = ProgramStatusRecords::default();
    records.apply(parse_body("state=working:app=x:msg=YQ").unwrap(), 1);
    records.apply(parse_body("state=working").unwrap(), 2);
    let root = records.root().unwrap();
    assert_eq!(root.app, None);
    assert_eq!(root.msg, None);
}

#[test]
fn clear_removes_descendants() {
    let mut records = ProgramStatusRecords::default();
    for body in [
        "state=working",
        "state=working:id=a",
        "state=working:id=a/b",
        "state=working:id=ab",
        "state=working:id=c",
    ] {
        records.apply(parse_body(body).unwrap(), 1);
    }
    records.apply(parse_body("state=clear:id=a").unwrap(), 2);
    let ids: Vec<Option<String>> = records.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids, vec![None, Some("ab".to_owned()), Some("c".to_owned())]);
    records.apply(parse_body("state=clear").unwrap(), 3);
    assert!(records.is_empty());
}

#[test]
fn app_is_inherited_from_the_nearest_ancestor() {
    let mut records = ProgramStatusRecords::default();
    records.apply(parse_body("state=working:app=deploy").unwrap(), 1);
    records.apply(parse_body("state=working:id=a:app=inner").unwrap(), 1);
    records.apply(parse_body("state=blocked:id=a/b/c").unwrap(), 1);
    records.apply(parse_body("state=blocked:id=z").unwrap(), 1);
    assert_eq!(
        records.inherited_app(&Some("a/b/c".to_owned())).as_deref(),
        Some("inner")
    );
    assert_eq!(
        records.inherited_app(&Some("z".to_owned())).as_deref(),
        Some("deploy")
    );
}

#[test]
fn least_recently_updated_record_is_evicted() {
    let mut records = ProgramStatusRecords::default();
    for i in 0..MAX_RECORDS {
        records.apply(parse_body(&format!("state=working:id=r{}", i)).unwrap(), 1);
    }
    records.apply(parse_body("state=done:id=r0").unwrap(), 2);
    records.apply(parse_body("state=working:id=new").unwrap(), 3);
    assert_eq!(records.len(), MAX_RECORDS);
    assert!(records.get("r0").is_some());
    assert!(records.get("r1").is_none());
    assert!(records.get("new").is_some());
}

#[test]
fn transient_records_are_dropped() {
    let mut records = ProgramStatusRecords::default();
    records.apply(parse_body("state=working").unwrap(), 1);
    records.apply(parse_body("state=blocked:id=a").unwrap(), 1);
    records.apply(parse_body("state=idle:id=b").unwrap(), 1);
    records.apply(parse_body("state=done:id=c").unwrap(), 1);
    records.apply(parse_body("state=error:id=d").unwrap(), 1);
    records.drop_transient();
    let ids: Vec<Option<String>> = records.iter().map(|r| r.id.clone()).collect();
    assert_eq!(ids, vec![Some("c".to_owned()), Some("d".to_owned())]);
}

#[test]
fn children_raise_the_pane_status() {
    let mut records = ProgramStatusRecords::default();
    records.apply(parse_body("state=working:app=deploy").unwrap(), 1);
    records.apply(parse_body("state=blocked:id=eu:msg=YQ").unwrap(), 1);
    let (status, record) = records.deciding_record().unwrap();
    assert_eq!(status, AttentionStatus::NeedsYou);
    assert_eq!(record.id.as_deref(), Some("eu"));
    assert_eq!(record.app.as_deref(), Some("deploy"));
    records.apply(parse_body("state=clear:id=eu").unwrap(), 2);
    let (status, record) = records.deciding_record().unwrap();
    assert_eq!(status, AttentionStatus::Working);
    assert_eq!(record.id, None);
}

#[test]
fn a_finished_program_remembers_how_long_it_worked() {
    let mut records = ProgramStatusRecords::default();
    records.apply(parse_body("state=working").unwrap(), 1_000);
    records.apply(parse_body("state=blocked:kind=question").unwrap(), 4_000);
    records.apply(parse_body("state=working").unwrap(), 6_000);
    assert_eq!(records.worked_ms(&None), None);
    records.apply(parse_body("state=done").unwrap(), 13_000);
    assert_eq!(records.worked_ms(&None), Some(12_000));
    records.apply(parse_body("state=done").unwrap(), 20_000);
    assert_eq!(
        records.worked_ms(&None),
        Some(12_000),
        "repeating done does not restart the clock"
    );
    records.apply(parse_body("state=working:id=a").unwrap(), 30_000);
    records.apply(parse_body("state=error:id=a").unwrap(), 30_500);
    assert_eq!(records.worked_ms(&Some("a".to_owned())), Some(500));
    records.apply(parse_body("state=done:id=b").unwrap(), 31_000);
    assert_eq!(
        records.worked_ms(&Some("b".to_owned())),
        None,
        "a program that never said it was working has no known duration"
    );
    records.apply(parse_body("state=working").unwrap(), 40_000);
    assert_eq!(records.worked_ms(&None), None);
    records.clear_all();
    records.apply(parse_body("state=done").unwrap(), 50_000);
    assert_eq!(records.worked_ms(&None), None);
}

#[test]
fn snapshots_preserve_update_order_and_reject_invalid_fields() {
    let mut records = ProgramStatusRecords::default();
    records.apply(parse_body("state=working:id=z").unwrap(), 1);
    records.apply(parse_body("state=done:id=a").unwrap(), 2);
    let snapshot = records.snapshot();
    assert_eq!(snapshot[0].id.as_deref(), Some("z"));
    let restored = ProgramStatusRecords::restore(snapshot.clone()).unwrap();
    assert_eq!(restored.snapshot(), snapshot);
    let mut invalid = snapshot;
    invalid[0].msg = Some("bad\nmessage".into());
    assert!(ProgramStatusRecords::restore(invalid).is_err());
}

#[test]
fn matching_candidates_resolve_apps_before_attention_selection() {
    let mut records = ProgramStatusRecords::default();
    for body in [
        "state=working:app=codex",
        "state=blocked:id=tool:app=terraform",
        "state=idle:id=tool/nested",
    ] {
        records.apply(parse_body(body).unwrap(), 1);
    }
    let compatible = |r: &ProgramStatusRecord| r.app.as_deref() == Some("codex");
    let (_, root) = records.deciding_record_matching(compatible).unwrap();
    assert_eq!(root.id, None);
    records.apply(parse_body("state=blocked:id=tool/nested").unwrap(), 2);
    assert_eq!(
        records.deciding_record_matching(compatible).unwrap().1.id,
        None
    );
    records.apply(parse_body("state=blocked:id=worker").unwrap(), 3);
    let (_, child) = records.deciding_record_matching(compatible).unwrap();
    assert_eq!(child.id.as_deref(), Some("worker"));
    assert_eq!(child.app.as_deref(), Some("codex"));
    assert!(records.deciding_record_matching(|_| false).is_none());
    assert_eq!(records.all().len(), 4);
}

#[test]
fn borrowed_selection_preserves_priority_apps_and_ties() {
    let mut records = ProgramStatusRecords::default();
    for body in [
        "state=working:app=codex",
        "state=blocked:id=tool:app=terraform",
        "state=blocked:id=tool/nested",
        "state=error:id=a",
        "state=error:id=b",
    ] {
        records.apply(parse_body(body).unwrap(), 1);
    }
    let select = |records: &ProgramStatusRecords| {
        let (_, record) = records
            .deciding_record_matching_ref(|_, app| app == Some("codex"))
            .unwrap();
        assert!(std::ptr::eq(
            record,
            records.get(record.id.as_deref().unwrap_or("")).unwrap()
        ));
        record.id.clone()
    };
    assert_eq!(select(&records).as_deref(), Some("a"));
    records.apply(parse_body("state=blocked:id=b").unwrap(), 2);
    assert_eq!(select(&records).as_deref(), Some("b"));
    records.apply(parse_body("state=blocked:app=codex").unwrap(), 3);
    assert_eq!(select(&records), None);
    assert!(records.deciding_record_matching_ref(|_, _| false).is_none());
}
