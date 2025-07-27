use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha1_smol::Sha1;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::Path;

use crate::instance::paths::InstancePaths;
use crate::instance::Instance;

pub fn recreate<P>(file: &P) -> Result<(File, Value), String>
where
    P: AsRef<Path>
{
    let instaces_list_default_struct: serde_json::Value = json!({});

    match File::create(file) {
        Ok(mut file) => {
            file.write_all(
                serde_json::to_string_pretty(&instaces_list_default_struct)
                    .unwrap()
                    .as_bytes(),
            )
            .unwrap();
            Ok((file, instaces_list_default_struct))
        }
        Err(e) => Err(format!("Failed to create instance manifest file: {}", e)),
    }
}

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

pub fn gen_manifest<'a>(
    instance: &Instance,
    paths: &InstancePaths, // Instance manifest file path
) -> Result<(), String> {
    let instance_manifest_file = match OpenOptions::new()
        .read(true)
        .write(false)
        .create(false)
        .open(paths.instance_manifest_file())
    {
        Ok(file) => {
            println!("Instance manifest found");
            file
        }

        Err(e) => {
            if e.kind() == ErrorKind::NotFound {
                match recreate(paths.instance_manifest_file()) {
                    Ok(_) => {
                        match OpenOptions::new()
                            .read(true)
                            .write(false)
                            .create(false)
                            .open(paths.instance_manifest_file())
                        {
                            Ok(file) => file,
                            Err(e) => return Err(e.to_string()),
                        }
                    }

                    Err(e) => {
                        return Err(format!("Failed to create instance manifest file: {}", e))
                    }
                }
            } else {
                return Err(format!("Failed to open instance manifest file: {}", e));
            }
        }
    };

    let mut _instance_manifest: serde_json::Value =
        match serde_json::from_reader(&instance_manifest_file) {
            Ok(value) => value,
            Err(_) => json!({}),
        };

    let instance_version = instance.version().as_ref().unwrap();
    let instance_name = &instance.name;

    // Generate UUID for instance
    let mut hasher = Sha1::new();
    let hasher_ready_input = format!("{}_{}", instance_name, instance_version);
    hasher.update(hasher_ready_input.as_bytes());
    let uuid = hasher.digest().to_string();

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
    ).unwrap();

    instance_manifest_file
        .write_all(
            serde_json::to_string_pretty(
                &serde_json::to_value(config).unwrap()
            )
                .unwrap()
                .as_bytes(),
        )
        .unwrap();

    Ok(())
}
