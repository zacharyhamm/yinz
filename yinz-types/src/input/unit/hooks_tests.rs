use super::*;

#[test]
fn the_default_hooks_bind_the_shipped_plugins() {
    let hooks = Hooks::default();
    assert_eq!(
        hooks.get(HookPoint::AgentExit).map(|p| p.location_string()),
        Some("yinz:agent-exit".to_owned()),
        "a session with no config still shows the agent-exit screen"
    );
    assert_eq!(
        hooks
            .get(HookPoint::ContextMenu)
            .map(|p| p.location_string()),
        Some("yinz:context-menu".to_owned()),
        "and still has a menu to offer, once the option turns it on"
    );
}

#[test]
fn a_hook_point_round_trips_through_its_name() {
    for point in HookPoint::ALL {
        assert_eq!(HookPoint::from_str(point.as_str()).as_ref(), Ok(point));
    }
}

#[test]
fn an_unknown_hook_point_names_the_ones_that_exist() {
    // `agent_started` is the near miss for `agent_start`: the message has to list the real
    // name next to the wrong one.
    let err = HookPoint::from_str("agent_started").unwrap_err();
    assert!(err.contains("agent_started"), "{err}");
    assert!(err.contains("agent_exit"), "{err}");
    assert!(err.contains("agent_start,"), "{err}");
}

#[test]
fn a_dashed_spelling_is_the_same_point() {
    for point in HookPoint::ALL {
        let dashed = point.as_str().replace('_', "-");
        assert_eq!(HookPoint::from_str(&dashed).as_ref(), Ok(point));
    }
}

#[test]
fn each_point_has_a_shape() {
    assert_eq!(HookPoint::AgentExit.shape(), HookShape::ReplacePane);
    for point in HookPoint::ALL {
        if *point != HookPoint::AgentExit {
            assert_eq!(point.shape(), HookShape::Headless, "{point}");
        }
    }
}

#[test]
fn hook_point_serializes_as_its_name() {
    // What `Event::Hook` carries to a plugin is the spelling the config uses.
    for point in HookPoint::ALL {
        let json = serde_json::to_string(point).unwrap();
        assert_eq!(json, format!("\"{}\"", point.as_str()));
        assert_eq!(serde_json::from_str::<HookPoint>(&json).unwrap(), *point);
    }
}

#[test]
fn merging_replaces_a_bound_point_and_leaves_the_others() {
    let mut hooks = Hooks::default();
    let mut other = Hooks::from_data(BTreeMap::new());
    other.set(
        HookPoint::AgentExit,
        RunPluginOrAlias::RunPlugin(RunPlugin {
            location: RunPluginLocation::File("/tmp/mine.scm".into()),
            ..Default::default()
        }),
    );
    hooks.merge(other);
    assert_eq!(
        hooks.get(HookPoint::AgentExit).map(|p| p.location_string()),
        Some("file:/tmp/mine.scm".to_owned())
    );
}

#[test]
fn clearing_a_point_unbinds_it_and_leaves_the_others() {
    let mut hooks = Hooks::default();
    hooks.clear(HookPoint::AgentExit);
    assert!(hooks.get(HookPoint::AgentExit).is_none());
    assert!(
        hooks.get(HookPoint::ContextMenu).is_some(),
        "clearing one point says nothing about any other"
    );

    for point in HookPoint::ALL {
        hooks.clear(*point);
    }
    assert!(hooks.is_empty());
}
