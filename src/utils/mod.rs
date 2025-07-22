use std::path::PathBuf;
use std::env;

use async_std::path::Path;
use home::home_dir;

use crate::data::definitions::EnvVars;

pub mod download;
pub mod instance_manifest;
pub mod instances_list;
pub mod metacache;


pub fn extract_filename(path: &str) -> Option<&str> {
    let last_slash = path.rfind('/')?;
    let last_dot = path.rfind('.')?;

    if last_slash < last_dot {
        Some(&path[last_slash + 1..last_dot])
    } else {
        None
    }
}

pub async fn get_home_dir() -> Option<PathBuf> {
    let home_dir_env_key = EnvVars::as_str(&EnvVars::HomeDirOverride);

    match env::var(home_dir_env_key) {
        Ok(val) => {
            let exists = Path::new(&val).exists().await;
            if exists {
                println!("Using home dir override: {}", val);
                return Some(PathBuf::from(val));
            }
        },
        Err(e) => {
            match e {
                env::VarError::NotUnicode(_) => {
                    eprintln!("Environment variable {} is not unicode.", home_dir_env_key);
                },
                _ => ()
            }
        }
    };

    match home_dir() {
        Some(path) => {
            return Some(PathBuf::from(path));
        },
        None => {
            eprintln!("Couldn't determine the home dir");
            return None;
        }
    };
}
