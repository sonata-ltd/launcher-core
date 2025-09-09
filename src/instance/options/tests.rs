use crate::instance::options::pages::overview::ExportTypes;

use super::*;

// #[test]
// fn export_bindings() -> Result<(), Box<dyn std::error::Error>> {
//     let out = concat!(env!("CARGO_MANIFEST_DIR"), "/bindings/instance/options");
//     fs::create_dir_all(out)?;

//     OverviewFields::export_all_to(out)?;
//     SettingsFields::export_all_to(out)?;

//     Ok(())
// }

#[test]
fn parse_change_request_partially() {
    let raw = r#"
        {
            "id": "8",
            "page": "overview",
            "options": {
                "name": "a new name",
                "tags": "Fabric, SMP"
            }
        }
    "#;

    let req_builder: ChangeRequestBuilder = serde_json::from_str(raw).unwrap();
    let req_builded = req_builder.build().unwrap();

    assert_eq!(req_builded.id, 8 as i64);
    assert!(matches!(req_builded.change, ChangableOptions::Overview(_)));
    match req_builded.change {
        ChangableOptions::Overview(fields) => {
            assert_eq!(fields.name, Some(String::from("a new name")));
            assert_eq!(fields.tags, Some(String::from("Fabric, SMP")));
            assert!(fields.export_type.is_none());
            assert!(fields.playtime.is_none());
        },
        _ => {
            panic!("Expected OverviewFields with 'name' and 'tags'");
        }
    }
}

#[test]
fn parse_change_request_fully() {
    let raw = r#"
        {
            "id": "14",
            "page": "overview",
            "options": {
                "name": "a new name",
                "tags": "Fabric, SMP",
                "export_type": "Sonata",
                "playtime": 512
            }
        }
    "#;

    let req_builder: ChangeRequestBuilder = serde_json::from_str(raw).unwrap();
    let req_builded = req_builder.build().unwrap();

    assert_eq!(req_builded.id, 14 as i64);
    assert!(matches!(req_builded.change, ChangableOptions::Overview(_)));
    match req_builded.change {
        ChangableOptions::Overview(fields) => {
            assert_eq!(fields.name, Some(String::from("a new name")));
            assert_eq!(fields.tags, Some(String::from("Fabric, SMP")));
            assert_eq!(fields.export_type, Some(ExportTypes::Sonata));
            assert_eq!(fields.playtime, Some(512 as i64));
        },
        _ => {
            panic!("Expected OverviewFields with 'name' and 'tags'");
        }
    }
}

#[test]
fn parse_change_request_empty() {
    let raw = r#"
        {
            "id": "14",
            "page": "overview",
            "options": {}
        }
    "#;

    let req_builder: ChangeRequestBuilder = serde_json::from_str(raw).unwrap();
    let req_builded = req_builder.build().unwrap();

    assert_eq!(req_builded.id, 14 as i64);
    assert!(matches!(req_builded.change, ChangableOptions::Overview(_)));
    match req_builded.change {
        ChangableOptions::Overview(fields) => {
            assert!(fields.name.is_none());
            assert!(fields.tags.is_none());
            assert!(fields.export_type.is_none());
            assert!(fields.playtime.is_none());
        },
        _ => {
            panic!("Expected OverviewFields with 'name' and 'tags'");
        }
    }
}

#[test]
fn parse_change_request_wrong_page() {
    let raw = r#"
        {
            "id": "14",
            "page": "asdq",
            "options": {}
        }
    "#;

    let req_builder: ChangeRequestBuilder = serde_json::from_str(raw).unwrap();
    let req_builded = req_builder.build();

    if let Err(e) = req_builded {
        assert_eq!(e.to_string(), "Wrong options page is present: asdq".to_string());
    } else {
        panic!("Should return error about incorrect page");
    }
}

#[test]
fn parse_change_request_wrong_id() {
    let raw = r#"
        {
            "id": "wrongid",
            "page": "overview",
            "options": {}
        }
    "#;

    let req_builder: ChangeRequestBuilder = serde_json::from_str(raw).unwrap();
    let req_builded = req_builder.build();

    if let Err(e) = req_builded {
        assert_eq!(e.to_string(), "Instance id is wrong: must be integer (i64), got \"wrongid\"".to_string());
    } else {
        panic!("Should return error about incorrect id");
    }
}

#[test]
fn parse_change_requets_different_case() {
    let raw = r#"
        {
            "id": "623",
            "page": "oVerVieW",
            "options": {}
        }
    "#;

    let req_builder: ChangeRequestBuilder = serde_json::from_str(raw).unwrap();
    let req_builded = req_builder.build().unwrap();

    assert!(matches!(req_builded.change, ChangableOptions::Overview(_)));
}
