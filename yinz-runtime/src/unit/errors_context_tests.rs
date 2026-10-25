use super::*;

#[test]
fn pure_context_changes_only_publish_when_requested() {
    let original = get_current_ctx();
    update_thread_ctx(&ErrorContext::new());
    let mut context = ErrorContext::new();
    context.add_call(ContextType::AsyncTask);
    assert!(get_current_ctx().is_empty());
    update_thread_ctx(&context);
    assert_eq!(get_current_ctx().calls(), context.calls());
    add_call(&mut context, ContextType::StdinHandler);
    assert_eq!(get_current_ctx().calls(), context.calls());
    let rendered = format_context(&context);
    assert!(rendered.starts_with("Originating Thread(s)\n\t\u{1b}[0;0m1. "));
    assert!(rendered.contains("AsyncTask"));
    assert!(rendered.contains("AcceptInput"));
    update_thread_ctx(&original);
}
