use crate::modloader::error::{ModloaderError, Result};
use serde_json::Value;
use std::collections::HashMap;

pub use crate::instance::rules::{rules_allow, RuleContext};

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
