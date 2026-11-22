use super::*;
use std::collections::BTreeMap;

const GENERATED: &str = include_str!("../../../assets/prost_ipc/client_server_contract.rs");

fn after<'a>(text: &'a str, marker: &str) -> &'a str {
    let start = text
        .find(marker)
        .unwrap_or_else(|| panic!("missing {marker}"));
    &text[start + marker.len()..]
}

fn tag(attribute: &str) -> u32 {
    let rest = after(attribute, "tag=\"");
    rest[..rest.find('"').unwrap()].parse().unwrap()
}

/// The envelope reads outer fields by number before prost decodes them, so its table must
/// track the generated schema. A merge can add a oneof arm or reuse the owner's number
/// without touching this file.
#[test]
fn attachment_generation_envelope_fields_match_the_generated_schema() {
    for (direction, message, module) in [
        (
            Direction::Client,
            "ClientToServerMsg",
            "client_to_server_msg",
        ),
        (
            Direction::Server,
            "ServerToClientMsg",
            "server_to_client_msg",
        ),
    ] {
        let fields = after(GENERATED, &format!("pub struct {message} {{"));
        let fields = &fields[..fields.find("\n}\n").unwrap()];
        let owner = fields
            .lines()
            .zip(fields.lines().skip(1))
            .find(|(_, next)| next.contains("pub attachment_generation:"))
            .map(|(attribute, _)| tag(attribute))
            .unwrap();
        assert_eq!(direction.owner_field(), owner, "{message} owner field");

        let arms = after(
            after(GENERATED, &format!("pub mod {module} {{")),
            "pub enum Message {",
        );
        let arms = &arms[..arms.find("\n    }\n").unwrap()];
        let mut expected = BTreeMap::new();
        for attribute in arms
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("#[prost("))
        {
            let kind = &attribute["#[prost(".len()..attribute.find(',').unwrap()];
            let wire = match kind {
                "bool" | "uint32" | "uint64" | "int32" | "int64" | "enumeration" => {
                    WireType::Varint
                },
                "message" | "string" | "bytes" => WireType::LengthDelimited,
                other => panic!("{message}: unclassified oneof kind {other}"),
            };
            assert!(expected.insert(tag(attribute), wire).is_none());
        }
        assert!(!expected.contains_key(&owner), "{message} owner collides");
        let highest = expected.keys().copied().chain([owner]).max().unwrap();
        for field in 1..=highest + 8 {
            assert_eq!(
                direction.message_wire_type(field),
                expected.get(&field).copied(),
                "{message} field {field}"
            );
        }
    }
}
