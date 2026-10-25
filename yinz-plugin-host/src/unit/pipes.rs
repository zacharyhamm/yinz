use super::*;

fn complete(
    plugin: PluginId,
    instance: PluginInstanceId,
    request: PipeRequestId,
    change: PipeStateChange,
    result: Result<(), String>,
) -> PipeCompletion {
    PipeCompletion {
        plugin_id: plugin,
        instance_id: instance,
        changes: HashMap::from([(request, change)]),
        callback: Some((request, result)),
    }
}

#[test]
fn stale_results_cannot_complete_another_input_on_the_same_stream() {
    let mut pending = PendingPipes::default();
    let plugin = PluginId::new(1);
    let instance = PluginInstanceId::default();
    let first = pending.begin("stream");
    pending.processing(first, plugin, instance);
    let old = complete(plugin, instance, first, PipeStateChange::NoChange, Ok(()));
    pending.complete(old.clone());
    assert_eq!(pending.take_ready(), [("stream".into(), Ok(()))]);
    let second = pending.begin("stream");
    pending.processing(second, plugin, instance);
    pending.complete(old);
    pending.retire(plugin, PluginInstanceId::default(), "stale retirement");
    assert!(pending.take_ready().is_empty());
    pending.complete(complete(
        plugin,
        instance,
        second,
        PipeStateChange::Block,
        Ok(()),
    ));
    assert!(pending.take_ready().is_empty());
    pending.complete(complete(
        plugin,
        instance,
        first,
        PipeStateChange::Unblock,
        Ok(()),
    ));
    assert!(pending.take_ready().is_empty());
    pending.complete(complete(
        plugin,
        instance,
        second,
        PipeStateChange::Unblock,
        Ok(()),
    ));
    assert_eq!(pending.take_ready(), [("stream".into(), Ok(()))]);
}

#[test]
fn failed_fanout_waits_for_callbacks_and_has_no_later_success() {
    let mut pending = PendingPipes::default();
    let plugin = PluginId::new(1);
    let first = PluginInstanceId::default();
    let peer = PluginInstanceId::default();
    let request = pending.begin("fanout");
    for instance in [first, peer] {
        pending.processing(request, plugin, instance);
    }
    pending.complete(complete(
        plugin,
        first,
        request,
        PipeStateChange::Unblock,
        Err("callback failed".into()),
    ));
    assert!(pending.take_ready().is_empty());
    pending.retire(plugin, first, "retired");
    assert!(pending.take_ready().is_empty());
    pending.complete(complete(
        plugin,
        peer,
        request,
        PipeStateChange::NoChange,
        Ok(()),
    ));
    assert_eq!(
        pending.take_ready(),
        [("fanout".into(), Err("callback failed".into()))]
    );
    pending.retire(plugin, peer, "retired");
    assert!(pending.take_ready().is_empty());
}

#[test]
fn replay_and_retirement_must_supply_an_outcome() {
    let mut pending = PendingPipes::default();
    let plugin = PluginId::new(1);
    let instance = PluginInstanceId::default();
    let request = pending.begin("replay");
    pending.cache(request, plugin, 2);
    pending.reject_cached(plugin, Some(1), "old load failed");
    assert!(pending.take_ready().is_empty());
    pending.processing(request, plugin, instance);
    pending.finish_cached(request, plugin, 2, Ok(()));
    pending.complete(complete(
        plugin,
        instance,
        request,
        PipeStateChange::Block,
        Ok(()),
    ));
    assert!(pending.take_ready().is_empty());
    pending.retire(plugin, instance, "hold interrupted");
    assert_eq!(
        pending.take_ready(),
        [("replay".into(), Err("hold interrupted".into()))]
    );
}
