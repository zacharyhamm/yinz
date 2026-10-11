use super::*;

fn snapshot(definitions: Vec<Definition>) -> AgentThemeSnapshot {
    AgentThemeSnapshot {
        version: SNAPSHOT_VERSION,
        provider: "fixture".into(),
        provider_version: "1".into(),
        theme: "fixture".into(),
        provenance: vec![],
        availability: Availability::Available,
        definitions,
        collisions: vec![],
    }
}

fn definition(token: &str, channel: Channel, role: Option<Role>) -> Definition {
    Definition {
        token: token.into(),
        channel,
        color: SourceColor::Index(2),
        pair: None,
        target: role.map(Into::into),
    }
}

#[test]
fn channels_and_equivalent_aliases_do_not_collide() {
    let s = snapshot(vec![
        definition("success", Channel::Foreground, Some(Role::Success)),
        definition("addition", Channel::Foreground, Some(Role::Success)),
        definition("surface", Channel::Background, Some(Role::Surface)),
    ]);
    let (mapping, collisions) = s.compile();
    assert!(collisions.is_empty());
    assert_eq!(mapping.len(), 2);
}

#[test]
fn unsupported_tokens_block_mapping_and_order_does_not_matter() {
    let mut s = snapshot(vec![
        definition("success", Channel::Foreground, Some(Role::Success)),
        definition("identity", Channel::Foreground, None),
    ]);
    let result = s.compile();
    assert!(result.0.values().all(Option::is_none));
    assert_eq!(result.1[0].tokens, ["identity", "success"]);
    s.definitions.reverse();
    assert_eq!(s.compile(), result);
}

#[test]
fn explicit_pairs_disambiguate_singles_but_conflicting_pairs_stay_unresolved() {
    let mut pair = definition("added", Channel::Foreground, Some(Role::Success));
    pair.pair = Some((SourceColor::Index(2), SourceColor::Index(22)));
    let mut s = snapshot(vec![
        pair.clone(),
        definition("accent", Channel::Foreground, Some(Role::Accent)),
    ]);
    let (mapping, _) = s.compile();
    assert_eq!(
        mapping[&MatchKey {
            channel: pair.channel,
            color: pair.color,
            pair: pair.pair
        }],
        pair.target
    );
    assert_eq!(
        mapping[&MatchKey {
            channel: pair.channel,
            color: pair.color,
            pair: None
        }],
        None
    );
    pair.target = Some(Role::Error.into());
    pair.token = "conflict".into();
    s.definitions.push(pair);
    assert!(s.compile().0.values().all(Option::is_none));
}
