use std::{
    env,
    path::{Path, PathBuf},
};

use home::home_dir;

pub mod download;
pub mod maven;

pub async fn get_home_dir() -> Option<PathBuf> {
    match env::var("SONATA_MC_HOME") {
        Ok(val) => {
            let exists = Path::new(&val).exists();
            if exists {
                tracing::info!("using home directory override: {}", val);
                return Some(PathBuf::from(val));
            }
        }
        Err(e) => match e {
            env::VarError::NotUnicode(_) => {
                tracing::error!("environment variable {} is not unicode", "SONATA_MC_HOME");
            }
            _ => (),
        },
    };

    match home_dir() {
        Some(path) => {
            return Some(PathBuf::from(path));
        }
        None => {
            tracing::error!("couldn't determine the home directory");
            return None;
        }
    };
}

pub fn str_nth_occurrence(s: &str, pat: char, n: usize) -> Option<usize> {
    s.char_indices()
        .filter(|(_, c)| *c == pat)
        .nth(n - 1)
        .map(|(idx, _)| idx)
}
