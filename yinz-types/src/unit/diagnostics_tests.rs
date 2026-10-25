use super::*;

#[test]
fn context_preserves_its_wire_shape_and_bounded_stack() {
    let mut context = ErrorContext::new();
    assert!(context.is_empty());
    context.add_call(ContextType::Screen(ScreenContext::Render));
    let json = r#"{"calls":[{"Screen":"Render"},"Empty","Empty","Empty","Empty","Empty"]}"#;
    assert_eq!(serde_json::to_string(&context).unwrap(), json);
    let decoded: ErrorContext = serde_json::from_str(json).unwrap();
    assert_eq!(decoded.calls(), context.calls());
    for _ in 0..10 {
        context.add_call(ContextType::AsyncTask);
    }
    assert_eq!(context.calls().len(), 6);
    assert_eq!(
        context.calls()[0],
        ContextType::Screen(ScreenContext::Render)
    );
    assert!(context.calls()[1..]
        .iter()
        .all(|c| *c == ContextType::AsyncTask));
}
