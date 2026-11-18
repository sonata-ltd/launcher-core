use std::{
    env,
    path::{Path, PathBuf},
};

use home::home_dir;

pub mod cancellation;
pub mod download;
pub mod files;
pub(crate) mod jobs;
pub mod maven;
pub(crate) mod object_store;
pub(crate) mod platform;
pub(crate) mod process;
pub(crate) mod process_pin;
pub mod url;

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

/// Return the byte offset of a one-based occurrence. Zero has no occurrence.
pub fn str_nth_occurrence(s: &str, pat: char, n: usize) -> Option<usize> {
    s.char_indices()
        .filter(|(_, c)| *c == pat)
        .nth(n.checked_sub(1)?)
        .map(|(idx, _)| idx)
}

#[cfg(test)]
mod tests {
    use super::str_nth_occurrence;

    #[test]
    fn occurrence_uses_one_based_ordinals_and_utf8_byte_offsets() {
        assert_eq!(str_nth_occurrence("é/あ/", '/', 0), None);
        assert_eq!(str_nth_occurrence("é/あ/", '/', 1), Some(2));
        assert_eq!(str_nth_occurrence("é/あ/", '/', 2), Some(6));
        assert_eq!(str_nth_occurrence("é/あ/", '/', 3), None);
    }
}
