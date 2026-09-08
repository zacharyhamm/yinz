use super::*;
use deno_core::{JsRuntime, RuntimeOptions};
use zellij_types::data::PluginCommand;

fn convert(runtime: &mut JsRuntime, source: &str) -> Result<Value> {
    let value = runtime.execute_script("transport.js", source.to_owned())?;
    deno_core::scope!(scope, runtime);
    let value = v8::Local::new(scope, value);
    from_v8(scope, value)
}

#[test]
fn deno_transport_preserves_exact_integers_and_rejects_invalid_guards() {
    let mut runtime = JsRuntime::new(RuntimeOptions::default());
    for (source, expected) in [
        ("0", "0"),
        ("2147483647", "2147483647"),
        ("4294967295", "4294967295"),
        ("9007199254740991", "9007199254740991"),
        ("-9007199254740991", "-9007199254740991"),
        ("-9223372036854775808n", "-9223372036854775808"),
        ("9223372036854775807n", "9223372036854775807"),
        ("18446744073709551615n", "18446744073709551615"),
    ] {
        assert_eq!(
            convert(&mut runtime, source).unwrap().to_string(),
            expected,
            "{source}"
        );
    }
    for invalid in [
        "NaN",
        "Infinity",
        "-Infinity",
        "18446744073709551616n",
        "-9223372036854775809n",
    ] {
        assert!(
            convert(&mut runtime, &format!("({{expect_rev: {invalid}}})")).is_err(),
            "{invalid}"
        );
    }
    for invalid in ["1.5", "-1.5", "9007199254740992", "-9007199254740992"] {
        let value = convert(&mut runtime, invalid).unwrap();
        assert!(value.is_f64());
        assert!(serde_json::from_value::<Option<i64>>(value.clone()).is_err());
        assert!(serde_json::from_value::<f64>(value).is_ok());
    }
    for (input, valid) in [
        ("4294967295", true),
        ("4294967296", false),
        ("-1", false),
        ("1.5", false),
    ] {
        let value = convert(&mut runtime, &format!("({{CloseTerminalPane: {input}}})")).unwrap();
        assert_eq!(
            serde_json::from_value::<PluginCommand>(value).is_ok(),
            valid,
            "{input}"
        );
    }
    for (input, valid) in [
        ("-2147483649", false),
        ("-2147483648", true),
        ("2147483647", true),
        ("2147483648", false),
        ("2147483648n", false),
    ] {
        assert_eq!(
            serde_json::from_value::<i32>(convert(&mut runtime, input).unwrap()).is_ok(),
            valid,
            "{input}"
        );
    }
}

#[test]
fn deno_transport_accepts_data_and_refuses_executable_objects() {
    let mut runtime = JsRuntime::new(RuntimeOptions::default());
    assert!(matches!(
        serde_json::from_value::<PluginCommand>(
            convert(&mut runtime, "({GetPluginIds: null})").unwrap()
        )
        .unwrap(),
        PluginCommand::GetPluginIds
    ));
    assert_eq!(convert(&mut runtime, "({bytes: new Uint8Array([0, 128, 255]), nested: [null, true, {n: 9223372036854775807n}]})").unwrap(),
        serde_json::json!({"bytes": [0, 128, 255], "nested": [null, true, {"n": i64::MAX}]}));
    for input in [
        "({get x() { globalThis.touched = true; return 1; }})",
        "new Proxy({}, {ownKeys() { globalThis.touched = true; return []; }})",
        "(()=> { const a = []; a.push(a); return a; })()",
        "[undefined]",
        "({x: undefined})",
        "new Date()",
        "new Map()",
        "(()=>{})",
        "Symbol()",
        "(()=> { let a = null; for(let i=0;i<130;i++) a=[a]; return a; })()",
    ] {
        assert!(convert(&mut runtime, input).is_err(), "{input}");
    }
    assert_eq!(
        convert(&mut runtime, "globalThis.touched === true").unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn deno_byte_conversion_preserves_the_hosts_file_limit() {
    let mut runtime = JsRuntime::new(RuntimeOptions::default());
    let value = convert(&mut runtime,
        "({FsWrite: {path:'/host/full-file', contents:new Uint8Array(8 * 1024 * 1024), append:false, create_dirs:false}})").unwrap();
    let PluginCommand::FsWrite { contents, .. } = serde_json::from_value(value).unwrap() else {
        panic!("expected file write");
    };
    assert_eq!(contents.len(), 8 * 1024 * 1024);
    assert!(convert(&mut runtime, "new Uint8Array(16 * 1024 * 1024 + 1)").is_err());
}
