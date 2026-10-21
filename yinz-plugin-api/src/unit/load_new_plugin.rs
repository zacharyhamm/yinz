use super::*;
use prost::Message;

#[test]
fn load_new_plugin_round_trips_omitted_and_unicode_cwd() {
    for cwd in [None, Some(PathBuf::from("/projects/雪 and 🦀"))] {
        let mut json = serde_json::json!({"LoadNewPlugin": {
            "url": "file:./plugin.scm", "config": {},
            "load_in_background": false, "skip_plugin_cache": true
        }});
        if let Some(cwd) = &cwd {
            json["LoadNewPlugin"]["cwd"] = serde_json::json!(cwd);
        }
        let command: PluginCommand = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&command).unwrap(), json);
        let protobuf: ProtobufPluginCommand = command.clone().try_into().unwrap();
        let bytes = protobuf.encode_to_vec();
        let decoded: PluginCommand = ProtobufPluginCommand::decode(bytes.as_slice())
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(decoded, command);
        let PluginCommand::LoadNewPlugin { cwd: actual, .. } = decoded else {
            panic!("expected LoadNewPlugin");
        };
        assert_eq!(actual, cwd);
    }
}
