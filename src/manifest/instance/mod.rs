pub mod uuid;

use async_std::fs::File;
use serde::{Deserialize, Serialize};
use async_std::io::WriteExt;

use crate::instance::paths::InstancePaths;
use crate::instance::Instance;


#[derive(Debug, Serialize, Deserialize)]
pub struct Config<'a> {
    #[serde(borrow)]
    pub general: General<'a>,
    pub overview: Overview<'a>,
    pub java: Java<'a>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct General<'a> {
    pub id: &'a str,
    pub version: &'a str,
    pub loader: &'a str,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Overview<'a> {
    pub name: &'a str,
    pub tags: &'a str,
    pub selected_export_type: Option<&'a str>,
    pub playtime: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Java<'a> {
    pub binary_path: &'a str,
}

pub async fn gen_manifest<'a>(
    instance: &Instance,
    paths: &InstancePaths,
    uuid: &String
) -> Result<(), String> {
    let instance_version = instance.version_id();
    let instance_name = &instance.name;

    let config = Config {
        general: General {
            id: &uuid,
            loader: "vanilla",
            version: &instance_version,
        },
        overview: Overview {
            name: &instance_name,
            tags: "",
            selected_export_type: None,
            playtime: 0,
        },
        java: Java { binary_path: "" },
    };


    let mut instance_manifest_file = File::create(
        format!("{}/{}.json", paths.headers().display(), &uuid)
    ).await.unwrap();

    instance_manifest_file
        .write_all(
            serde_json::to_string_pretty(
                &serde_json::to_value(config).unwrap()
            )
                .unwrap()
                .as_bytes(),
        )
        .await
        .unwrap();

    Ok(())
}
