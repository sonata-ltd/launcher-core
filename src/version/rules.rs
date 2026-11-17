use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuleError {
    #[error("invalid rule: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid OS version regex: {0}")]
    Regex(#[from] regex::Error),

    #[error("unsupported rule action: {0}")]
    UnsupportedAction(String),
}

type Result<T> = std::result::Result<T, RuleError>;

pub struct RuleContext<'a> {
    pub os: &'a str,
    pub arch: &'a str,
    pub os_version: &'a str,
    pub features: HashMap<String, bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    action: String,
    os: Option<RuleOs>,
    features: Option<HashMap<String, bool>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleOs {
    name: Option<String>,
    arch: Option<String>,
    version: Option<String>,
}

fn normalize_arch(value: &str) -> &str {
    match value {
        "amd64" | "x86_64" => "x86_64",
        "arm64" | "aarch64" => "aarch64",
        "i386" | "i686" | "x86" => "x86",
        other => other,
    }
}

fn matches_rule(rule: &Rule, context: &RuleContext<'_>) -> Result<bool> {
    if let Some(os) = &rule.os {
        if os.name.as_deref().is_some_and(|name| name != context.os) {
            return Ok(false);
        }

        if os
            .arch
            .as_deref()
            .is_some_and(|arch| normalize_arch(arch) != normalize_arch(context.arch))
        {
            return Ok(false);
        }

        if let Some(pattern) = &os.version {
            let regex = Regex::new(pattern).map_err(|e| RuleError::Regex(e))?;

            if !regex.is_match(context.os_version) {
                return Ok(false);
            }
        }
    }

    if let Some(features) = &rule.features {
        if !features
            .iter()
            .all(|(key, expected)| context.features.get(key).copied().unwrap_or(false) == *expected)
        {
            return Ok(false);
        }
    }

    Ok(true)
}

pub fn rules_allow(values: &[Value], context: &RuleContext<'_>) -> Result<bool> {
    if values.is_empty() {
        return Ok(true);
    }

    let mut allowed = false;

    for value in values {
        let rule: Rule = serde_json::from_value(value.clone())?;

        let action = match rule.action.as_str() {
            "allow" => true,
            "disallow" => false,
            other => return Err(RuleError::UnsupportedAction(other.into())),
        };

        if matches_rule(&rule, context)? {
            allowed = action;
        }
    }

    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_rules_unknown_actions_and_invalid_regex_are_distinct() {
        let context = RuleContext {
            os: "linux",
            arch: "x86_64",
            os_version: "6.12",
            features: HashMap::new(),
        };
        assert!(matches!(
            rules_allow(&[json!({"action": "allow", "unknown": true})], &context),
            Err(RuleError::Json(_))
        ));
        assert!(matches!(
            rules_allow(&[json!({"action": "unknown"})], &context),
            Err(RuleError::UnsupportedAction(action)) if action == "unknown"
        ));
        assert!(matches!(
            rules_allow(
                &[json!({"action": "allow", "os": {"version": "["}})],
                &context
            ),
            Err(RuleError::Regex(_))
        ));
    }
}
