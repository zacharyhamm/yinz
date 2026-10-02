use super::*;
use yinz_tile::agent::provider::{self, AgentProvider};

#[test]
fn provider_refresh_preserves_valid_selections_and_errors() {
    fn select(state: &mut State, field: &str, value: &str) {
        assert!(state
            .dialog
            .form_mut()
            .select_by_id_mut(field)
            .unwrap()
            .select_option(value));
    }

    let owner = 901_612;
    let mut descriptor: AgentProvider = serde_json::from_value(serde_json::json!({
        "kind":"fixture-permission-refresh", "executable":"fixture-permission-refresh",
        "launch":{"permissions":{"skip":["--skip"]},"efforts":{"high":["--high"]}}
    }))
    .unwrap();
    let mut state = State::default();
    select(&mut state, AGENT, "Claude");
    state.changed(AGENT);
    select(&mut state, PERMISSION_MODE, "plan");
    select(&mut state, EFFORT, "high");
    state.error = Some("launch failed".into());

    // An unrelated registration, reload, or unload must not change Claude's mode.
    for _ in 0..2 {
        provider::register(owner, descriptor.clone()).unwrap();
        state.update(Event::AgentProvidersChanged(provider::providers()));
        assert_eq!(state.dialog.form().text(AGENT), "Claude");
        assert_eq!(state.dialog.form().text(EFFORT), "high");
        assert_eq!(state.error.as_deref(), Some("launch failed"));
        assert_eq!(
            spec(state.dialog.form()).permission_mode.as_deref(),
            Some("plan")
        );
    }
    assert!(provider::unregister_owner(owner));
    state.update(Event::AgentProvidersChanged(provider::providers()));
    assert_eq!(
        spec(state.dialog.form()).permission_mode.as_deref(),
        Some("plan")
    );

    provider::register(owner, descriptor.clone()).unwrap();
    state.update(Event::AgentProvidersChanged(provider::providers()));
    select(&mut state, AGENT, "Fixture-permission-refresh");
    state.changed(AGENT);
    select(&mut state, PERMISSION_MODE, "skip");
    select(&mut state, EFFORT, "high");
    state.error = Some("provider failed".into());
    provider::register(owner, descriptor.clone()).unwrap();
    state.update(Event::AgentProvidersChanged(provider::providers()));
    assert_eq!(
        spec(state.dialog.form()).permission_mode.as_deref(),
        Some("skip")
    );

    assert_eq!(state.dialog.form().text(EFFORT), "high");
    assert_eq!(state.error.as_deref(), Some("provider failed"));
    descriptor.launch.efforts.clear();
    descriptor.launch.permissions.clear();
    provider::register(owner, descriptor.clone()).unwrap();
    state.update(Event::AgentProvidersChanged(provider::providers()));
    assert_eq!(spec(state.dialog.form()).permission_mode, None);
    assert_eq!(spec(state.dialog.form()).effort, None);
    assert_eq!(state.error.as_deref(), Some("provider failed"));

    descriptor
        .launch
        .permissions
        .insert("skip".into(), vec!["--skip".into()]);
    descriptor
        .launch
        .efforts
        .insert("high".into(), vec!["--high".into()]);
    provider::register(owner, descriptor).unwrap();
    state.update(Event::AgentProvidersChanged(provider::providers()));
    select(&mut state, PERMISSION_MODE, "skip");
    select(&mut state, EFFORT, "high");
    assert!(provider::unregister_owner(owner));
    state.update(Event::AgentProvidersChanged(provider::providers()));
    assert_eq!(state.dialog.form().text(AGENT), "Codex");
    assert_eq!(spec(state.dialog.form()).effort, None);
    assert_eq!(state.error.as_deref(), Some("provider failed"));
    assert_eq!(spec(state.dialog.form()).permission_mode, None);

    // Even a shared permission name must reset when the user switches agents.
    select(&mut state, PERMISSION_MODE, "skip");
    select(&mut state, AGENT, "Claude");
    state.changed(AGENT);
    assert_eq!(spec(state.dialog.form()).permission_mode, None);
}
