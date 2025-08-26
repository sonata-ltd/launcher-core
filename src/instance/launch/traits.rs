use strum::Display;
use thiserror::Error;

pub enum StartupTraits {
    FirstThreadOnMacOS
}

#[derive(Debug, Display, Error)]
pub enum StartupTraitsError {
    TraitsNotFound
}

impl StartupTraits {
    pub fn extract(manifest: &serde_json::Value) -> Result<Vec<StartupTraits>, StartupTraitsError> {
        let mut matched_traits = Vec::new();

        if let Some(traits) = manifest.get("+traits").and_then(|v| v.as_array()) {
            for current_trait in traits {
                match current_trait.as_str() {
                    Some(current_trait) => {
                        match current_trait {
                            "FirstThreadOnMacOS" => {
                                #[cfg(target_os = "macos")]
                                matched_traits.push(StartupTraits::FirstThreadOnMacOS);
                            },
                            _ => continue
                        }
                    }
                    None => continue
                }
            }
        }

        if !matched_traits.is_empty() {
            return Ok(matched_traits);
        } else {
            return Err(StartupTraitsError::TraitsNotFound)
        }
    }
}
