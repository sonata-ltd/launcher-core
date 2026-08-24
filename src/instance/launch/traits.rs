#[derive(Debug, Clone, Copy)]
pub enum StartupTraits {
    FirstThreadOnMacOS,
}

impl StartupTraits {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "FirstThreadOnMacOS" => Some(Self::FirstThreadOnMacOS),
            _ => None,
        }
    }

    const fn applies_here(self) -> bool {
        match self {
            Self::FirstThreadOnMacOS => cfg!(target_os = "macos"),
        }
    }

    pub fn extract(manifest: &serde_json::Value) -> Vec<Self> {
        manifest
            .get("+traits")
            .and_then(|v| v.as_array())
            .map(|traits| {
                traits
                    .iter()
                    .filter_map(|t| t.as_str())
                    .filter_map(Self::parse)
                    .filter(|t| t.applies_here())
                    .collect()
            })
            .unwrap_or_default()
    }
}
