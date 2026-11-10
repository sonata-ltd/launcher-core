use crate::modloader::error::{ModloaderError, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

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
            let regex = Regex::new(pattern)
                .map_err(|e| ModloaderError::Invalid(format!("OS version regex: {e}")))?;

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
            other => return Err(ModloaderError::Unsupported(format!("rule action: {other}"))),
        };

        if matches_rule(&rule, context)? {
            allowed = action;
        }
    }

    Ok(allowed)
}

pub fn substitute(input: &str, variables: &HashMap<String, String>) -> Result<String> {
    let mut rest = input;
    let mut out = String::new();

    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);

        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .ok_or_else(|| ModloaderError::Invalid("unclosed runtime placeholder".into()))?;

        let key = &tail[..end];

        out.push_str(variables.get(key).ok_or_else(|| {
            ModloaderError::Invalid(format!("unknown runtime placeholder: {key}"))
        })?);

        rest = &tail[end + 1..];
    }

    out.push_str(rest);
    Ok(out)
}

pub fn flatten(
    values: &[Value],
    context: &RuleContext<'_>,
    variables: &HashMap<String, String>,
) -> Result<Vec<String>> {
    let mut output = Vec::new();
    for value in values {
        match value {
            Value::String(raw) => output.push(substitute(raw, variables)?),
            Value::Object(object) => {
                if object.keys().any(|key| key != "rules" && key != "value") {
                    return Err(ModloaderError::Unsupported("unknown argument field".into()));
                }

                let rules = object
                    .get("rules")
                    .and_then(Value::as_array)
                    .ok_or_else(|| ModloaderError::Invalid("argument has no rules array".into()))?;

                if !rules_allow(rules, context)? {
                    continue;
                }

                match object.get("value") {
                    Some(Value::String(raw)) => output.push(substitute(raw, variables)?),
                    Some(Value::Array(values)) => {
                        for raw in values {
                            output.push(substitute(
                                raw.as_str().ok_or_else(|| {
                                    ModloaderError::Invalid(
                                        "argument array contains non-string".into(),
                                    )
                                })?,
                                variables,
                            )?);
                        }
                    }
                    _ => {
                        return Err(ModloaderError::Invalid(
                            "invalid conditional argument".into(),
                        ))
                    }
                }
            }
            _ => return Err(ModloaderError::Invalid("invalid runtime argument".into())),
        }
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn linux() -> RuleContext<'static> {
        RuleContext {
            os: "linux",
            arch: "x86_64",
            os_version: "6.12",
            features: HashMap::new(),
        }
    }

    #[test]
    fn embedded_placeholders_keep_spaces_in_one_argument() {
        let vars = HashMap::from([("library_directory".into(), "/My Games/libs".into())]);
        assert_eq!(
            substitute("-Ddir=${library_directory}", &vars).unwrap(),
            "-Ddir=/My Games/libs"
        );
    }

    #[test]
    fn last_matching_rule_wins() {
        let rules = vec![
            json!({"action":"allow"}),
            json!({"action":"disallow", "os":{"name":"linux"}}),
        ];
        assert!(!rules_allow(&rules, &linux()).unwrap());
    }

    #[test]
    fn missing_feature_is_false_and_array_values_are_flattened() {
        let values = vec![
            json!({"rules":[{"action":"allow", "features":{"is_demo_user":true}}], "value":"--demo"}),
            json!({"rules":[{"action":"allow", "os":{"name":"linux"}}], "value":["--flag","value"]}),
        ];
        assert_eq!(
            flatten(&values, &linux(), &HashMap::new()).unwrap(),
            ["--flag", "value"]
        );
    }

    #[test]
    fn unknown_runtime_variable_is_an_error() {
        assert!(substitute("${missing}", &HashMap::new()).is_err());
    }
}
