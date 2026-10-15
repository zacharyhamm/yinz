use super::*;
use crate::plugin_data::{AttrDef, DataValue, EntityDef, FetchRequest, Mutation, Pred};
use serde_json::json;

#[test]
fn compact_blob_deserialization_preserves_legacy_serialization_and_generic_json() {
    let encoded = encode(&[0, 128, 255]).unwrap();
    let wire = json!({"Blob": encoded});
    let blob: DataValue = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(blob, DataValue::Blob(vec![0, 128, 255]));
    assert_eq!(
        serde_json::to_value(&blob).unwrap(),
        json!({"Blob": [0, 128, 255]})
    );
    assert_eq!(serde_json::from_value::<Value>(wire.clone()).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<DataValue>(json!({"Blob": [0, 128, 255]})).unwrap(),
        blob
    );
    for malformed in [
        json!({MARKER: "!"}),
        json!({MARKER: "AA==", "extra": true}),
        json!({MARKER: 7}),
        json!([256]),
        json!([-1]),
        json!([1.5]),
    ] {
        assert!(serde_json::from_value::<DataValue>(json!({"Blob": malformed})).is_err());
    }
}

#[test]
fn compact_blobs_are_accepted_in_schema_mutation_and_nested_predicate_fields() {
    let blob = DataValue::Blob(vec![0, 255]);
    let entity = EntityDef::new("files", 1)
        .attr(AttrDef::new("contents", crate::plugin_data::AttrKind::Blob).default(blob.clone()));
    let mut wire = serde_json::to_value(&entity).unwrap();
    wire["attrs"][0]["default"]["Blob"] = encode(&[0, 255]).unwrap();
    assert_eq!(serde_json::from_value::<EntityDef>(wire).unwrap(), entity);
    let request = FetchRequest::entity("files").filter(Pred::all([
        Pred::eq("contents", blob.clone()),
        !Pred::is_in("contents", [blob.clone()]),
    ]));
    let mut wire = serde_json::to_value(&request).unwrap();
    wire["filter"]["And"][0]["Eq"][1]["Blob"] = encode(&[0, 255]).unwrap();
    wire["filter"]["And"][1]["Not"]["In"][1][0]["Blob"] = encode(&[0, 255]).unwrap();
    assert_eq!(
        serde_json::from_value::<FetchRequest>(wire).unwrap(),
        request
    );
    let mutation = Mutation::put("files").set("contents", blob).build();
    let mut wire = serde_json::to_value(&mutation).unwrap();
    wire["Put"]["values"][0][1]["Blob"] = encode(&[0, 255]).unwrap();
    assert_eq!(serde_json::from_value::<Mutation>(wire).unwrap(), mutation);
}
