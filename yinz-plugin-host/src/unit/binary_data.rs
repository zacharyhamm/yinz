use super::*;
use yinz_types::plugin_data::{AttrDef, AttrKind, EntityDef};

#[test]
fn typed_rows_and_catalog_keep_blob_payloads_compact_and_metadata_exact() {
    let original = DataRows {
        columns: vec!["contents".into(), "literal".into()],
        rows: vec![vec![
            DataValue::Blob(vec![0, 128, 255]),
            DataValue::Text(r#"{"$yinz-bytes":"AA=="}"#.into()),
        ]],
        truncated: true,
        next_cursor: Some(yinz_types::internal_ids::DataRowId::new(7)),
    };
    let encoded = rows(&original).unwrap();
    assert!(encoded["rows"][0][0]["Blob"].is_object());
    assert_eq!(
        serde_json::from_value::<DataRows>(encoded).unwrap(),
        original
    );
    let catalog_original = vec![DataEntityInfo {
        namespace: "private".into(),
        owner_plugin: "file:plugin.scm".into(),
        def: EntityDef::new("files", 1)
            .attr(AttrDef::new("contents", AttrKind::Blob).default(DataValue::Blob(vec![0, 255]))),
    }];
    let encoded = catalog(catalog_original.clone()).unwrap();
    assert!(encoded[0]["def"]["attrs"][0]["default"]["Blob"].is_object());
    assert_eq!(
        serde_json::from_value::<Vec<DataEntityInfo>>(encoded).unwrap(),
        catalog_original
    );
}
