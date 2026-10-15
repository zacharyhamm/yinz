use crate::{binary, Runtime};
use serde_json::{json, Value};

#[test]
fn typed_binary_values_preserve_bytes_without_reinterpreting_json_objects() {
    let bytes = vec![0, 255, 128, 195, 40, 10, 0];
    let encoded = binary::encode(&bytes).unwrap();
    let expected = encoded.clone();
    let mut runtime = Runtime::new(move |command| {
        if command.get("FsRead").is_some() {
            Ok(encoded.clone())
        } else {
            Ok(command)
        }
    })
    .unwrap();
    runtime
        .eval("(define data (yinz-command (hash \"FsRead\" (hash))))")
        .unwrap();
    assert_eq!(
        runtime
            .eval("(list (bytevector? data) (bytes-length data) (bytes-ref data 1))")
            .unwrap()
            .values,
        ["(#true 7 255)"]
    );
    assert_eq!(runtime.value("data").unwrap(), json!(bytes));
    assert_eq!(
        runtime.eval("(yinz-decode-utf8 data)").unwrap().values[0],
        format!("{:?}", String::from_utf8_lossy(&bytes))
    );
    runtime.eval("(define (echo value) value)").unwrap();
    assert_eq!(
        runtime.invoke("echo", vec![expected.clone()]).unwrap(),
        expected
    );
    assert_eq!(
        runtime.invoke("echo", vec![json!([0, 1, 255])]).unwrap(),
        json!([0, 1, 255])
    );
    runtime
        .eval("(define seen #f) (define (update event) (set! seen event) #t)")
        .unwrap();
    runtime.update(json!({"WebRequestResult": [200, {"literal":"$yinz-bytes"}, binary::encode(&[]).unwrap(), {}]})).unwrap();
    assert_eq!(runtime.eval("(list (bytevector? (list-ref (hash-ref seen \"WebRequestResult\") 2)) (bytes-length (list-ref (hash-ref seen \"WebRequestResult\") 2)))").unwrap().values, ["(#true 0)"]);
}

#[test]
fn compact_commands_and_eight_mib_text_keep_their_supported_sizes() {
    let mut runtime = Runtime::new(|command| {
        let command = command
            .get("__yinz_runtime")
            .and_then(|value| value.get("command"))
            .unwrap_or(&command);
        if let Some(write) = command.get("FsWrite") {
            let bytes = binary::decode(&write["contents"])?;
            assert_eq!(bytes.len(), 8 * 1024 * 1024);
            assert_eq!(bytes.first(), Some(&255));
            assert_eq!(bytes.last(), Some(&255));
            Ok(json!(bytes.len()))
        } else {
            assert_eq!(
                command["MessageToPlugin"]["payload"]
                    .as_str()
                    .unwrap()
                    .len(),
                8 * 1024 * 1024
            );
            Ok(Value::Null)
        }
    })
    .unwrap();
    runtime
        .eval("(define data (make-bytes 8388608 255)) (yinz-trace #t)")
        .unwrap();
    assert_eq!(runtime.eval("(yinz-command (hash \"FsWrite\" (hash \"path\" \"/host/copy\" \"contents\" data)))").unwrap().values, ["8388608"]);
    // Ordinary JSON text is not constrained by the source-file or rendering budgets.
    runtime.eval("(define text (make-string 8388608 #\\x)) (define encoded (yinz-json-string text)) (define decoded (yinz-json-parse encoded))").unwrap();
    assert_eq!(
        runtime.eval("(string-length decoded)").unwrap().values,
        ["8388608"]
    );
    runtime
        .eval("(yinz-command (hash \"MessageToPlugin\" (hash \"payload\" decoded)))")
        .unwrap();
}

#[test]
fn invalid_compact_payloads_are_rejected_only_at_binary_positions() {
    assert!(binary::decode(&json!({"$yinz-bytes":"%%%"})).is_err());
    assert!(binary::decode(&json!({"$yinz-bytes":"AA==", "extra": true})).is_err());
    assert!(binary::decode(&json!([256])).is_err());
    let marker = json!({"$yinz-bytes":"%%%"});
    assert_eq!(
        crate::from_scheme(&crate::to_scheme(&marker).unwrap()).unwrap(),
        marker
    );
    let mut runtime = Runtime::new(|_| Ok(json!({"$yinz-bytes":"%%%"}))).unwrap();
    assert!(runtime
        .eval("(yinz-command (hash \"FsRead\" (hash)))")
        .is_err());
    assert_eq!(runtime.eval("(+ 1 2)").unwrap().values, ["3"]);
}

#[test]
fn legacy_project_parser_borrows_large_bytevector_without_integer_list_expansion() {
    let mut runtime = Runtime::new(|_| Ok(Value::Null)).unwrap();
    runtime.eval(r#"(define input (bytes-append (string->bytes "{\"agent\":{\"name\":\"agent\",\"root\":\"/work/agent\"}}") (make-bytes 200000 32)))
      (define registry (yinz-agent-model "projects" "ParseLegacy" (list input)))"#).unwrap();
    assert_eq!(
        runtime.value("registry").unwrap()["agent"]["root"],
        "/work/agent"
    );
}

#[test]
fn large_data_blobs_use_only_typed_data_positions_and_leave_text_json_unchanged() {
    use yinz_types::{
        data::PluginCommand,
        plugin_data::{DataValue, Mutation},
    };
    let size = 8 * 1024 * 1024;
    let blob = binary::encode(&vec![255; size]).unwrap();
    let rows = json!({"columns":["blob","text"], "rows":[[{"Blob":blob},{"Text":"{\"$yinz-bytes\":\"AA==\"}"}]], "truncated":false,"next_cursor":null});
    let watch_rows = rows.clone();
    let mut runtime = Runtime::new(move |command| {
        if command.get("DataFetch").is_some() {
            return Ok(rows.clone());
        }
        let PluginCommand::DataApply { mutations, .. } = binary::decode_command(command)? else {
            panic!("unexpected command");
        };
        let Mutation::Put { values, .. } = &mutations[0] else {
            panic!("unexpected mutation");
        };
        let DataValue::Blob(bytes) = &values[0].1 else {
            panic!("unexpected value");
        };
        assert_eq!(bytes.len(), size);
        assert!(bytes.iter().all(|byte| *byte == 255));
        Ok(Value::Null)
    })
    .unwrap();
    runtime.eval(r#"(define fetched (yinz-command (hash "DataFetch" (hash "request" (hash)))))
        (define row (car (hash-ref fetched "rows")))
        (define data (hash-ref (car row) "Blob"))
        (define text (hash-ref (cadr row) "Text"))
        (yinz-command (hash "DataApply" (hash "namespace" json-null "mutations"
          (list (hash "Put" (hash "entity" "blobs" "uid" "one" "values" (list (list "body" (hash "Blob" data))) "expect_rev" json-null))))))"#).unwrap();
    assert_eq!(
        runtime.eval("(bytes-length data)").unwrap().values,
        ["8388608"]
    );
    assert_eq!(
        runtime.value("text").unwrap(),
        json!("{\"$yinz-bytes\":\"AA==\"}")
    );
    runtime.eval("(define changed-size 0) (define (update event) (set! changed-size (bytes-length (hash-ref (car (car (hash-ref (hash-ref (hash-ref event \"PluginDataChanged\") \"rows\") \"rows\"))) \"Blob\"))) #t)").unwrap();
    runtime
        .update(json!({"PluginDataChanged":{"watch_id":1,"rows":watch_rows,"truncated":false}}))
        .unwrap();
    assert_eq!(runtime.value("changed-size").unwrap(), json!(size));
}

#[test]
fn recorded_commands_expand_only_typed_bytes_without_reordering_other_fields() {
    let subscribe = json!({"Subscribe": ["WebRequestResult", "Key", "Timer"]});
    assert_eq!(
        binary::expand_command_bytes(subscribe.clone()).unwrap(),
        subscribe
    );
    let user = json!({"MessageToPlugin": {"payload": {"$yinz-bytes":"AA=="}}});
    assert_eq!(binary::expand_command_bytes(user.clone()).unwrap(), user);
    assert_eq!(
        binary::expand_command_bytes(
            json!({"FsWrite": {"contents":binary::encode(&[0, 255]).unwrap(),"path":"/host/a"}})
        )
        .unwrap(),
        json!({"FsWrite":{"contents":[0,255],"path":"/host/a"}})
    );
}

#[test]
fn worker_event_dispatch_uses_the_same_typed_binary_boundary() {
    let mut runtime = Runtime::new(|_| Ok(Value::Null)).unwrap();
    runtime
        .eval(
            r#"(define received #f)
        (yinz-register-worker-events (lambda (event)
            (set! received (list-ref (hash-ref event "WebRequestResult") 2)) #t))"#,
        )
        .unwrap();
    runtime
        .invoke(
            "yinz-worker-event",
            vec![json!({"WebRequestResult":[200,{},binary::encode(&[0, 128, 255]).unwrap(),{}]})],
        )
        .unwrap();
    assert_eq!(
        runtime
            .eval("(list (bytevector? received) (bytes-length received) (bytes-ref received 2))")
            .unwrap()
            .values,
        ["(#true 3 255)"]
    );
}

#[test]
fn scoped_agent_models_borrow_large_binary_inputs_without_expanding_integer_lists() {
    let mut runtime = Runtime::new(|_| Ok(Value::Null)).unwrap();
    runtime.eval(r#"
        (define (model name . args) (yinz-agent-model "providers" "WithSnapshot" (list '() "agents" name args)))
        (define padding (make-bytes 200000 32))
        (define summary (bytes-append (string->bytes "{\"choices\":[{\"message\":{\"content\":\"{\\\"summary\\\":\\\"Tests pass.\\\",\\\"awaiting\\\":false}\"}}]}") padding))
        (define completion (bytes-append (string->bytes "{\"choices\":[{\"message\":{\"content\":\"Done.\"}}]}") padding))
        (define parsed-summary (model "ParseSummary" summary 7))
        (define parsed-completion (model "CompletionParse" 200 completion "request"))
        (define metadata (bytes-append (string->bytes "/work/tree\n/work/tree/.git\n/work/tree/.git\nmain\n") (make-bytes 200000 10)))
        (define parsed-git (model "GitProbe" 0 metadata (make-bytes 200000 0)))
    "#).unwrap();
    assert_eq!(
        runtime.value("parsed-summary").unwrap()["text"],
        "Tests pass."
    );
    assert_eq!(
        runtime.value("parsed-completion").unwrap()["content"],
        "Done."
    );
    assert!(runtime.value("parsed-git").unwrap()["Checkout"].is_object());
}

#[test]
fn remote_git_probe_replies_keep_large_bytes_compact_until_native_parsing() {
    let mut metadata = b"/work/tree\n/work/tree/.git\n/work/tree/.git\nmain\n".to_vec();
    metadata.extend(vec![b'\n'; 200000]);
    let reply = json!({"host":"ssh.example", "cwd":"/work/tree", "exit_code":0,
        "metadata":binary::encode(&metadata).unwrap(), "config":binary::encode(&vec![0; 200000]).unwrap()});
    let mut runtime = Runtime::new(move |_| Ok(reply.clone())).unwrap();
    runtime.eval(r#"(define probe (yinz-command (hash "GetPaneGitProbe" (hash))))
        (define result (yinz-agent-model "providers" "WithSnapshot" (list '() "agents" "GitProbe"
          (list (hash-ref probe "exit_code") (hash-ref probe "metadata") (hash-ref probe "config")))))"#).unwrap();
    assert_eq!(
        runtime
            .eval("(bytevector? (hash-ref probe \"metadata\"))")
            .unwrap()
            .values,
        ["#true"]
    );
    assert!(runtime.value("result").unwrap()["Checkout"].is_object());
}
