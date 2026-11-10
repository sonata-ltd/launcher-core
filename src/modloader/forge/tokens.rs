use crate::modloader::{
    error::{ModloaderError, Result},
    maven::MavenCoordinate,
};

use std::{collections::HashMap, path::Path};

pub fn expand_tokens(input: &str, tokens: &HashMap<String, String>) -> Result<String> {
    let mut chars = input.chars();
    let mut output = String::new();

    while let Some(c) = chars.next() {
        match c {
            '\\' => output.push(
                chars
                    .next()
                    .ok_or_else(|| ModloaderError::Invalid("bad installer escape".into()))?,
            ),
            '{' | '\'' => {
                let closing = if c == '{' { '}' } else { '\'' };

                let mut key = String::new();
                let mut closed = false;

                while let Some(next) = chars.next() {
                    if next == '\\' {
                        key.push(chars.next().ok_or_else(|| {
                            ModloaderError::Invalid("bad installer token escape".into())
                        })?);
                    } else if next == closing {
                        closed = true;
                        break;
                    } else {
                        key.push(next);
                    }
                }

                if !closed {
                    return Err(ModloaderError::Invalid(
                        "unclosed installer token/literal".into(),
                    ));
                }

                if c == '\'' {
                    output.push_str(&key);
                } else {
                    output.push_str(tokens.get(&key).ok_or_else(|| {
                        ModloaderError::Invalid(format!("unknown installer token: {key}"))
                    })?);
                }
            }
            _ => output.push(c),
        }
    }

    Ok(output)
}

pub fn resolve_argument(
    raw: &str,
    tokens: &HashMap<String, String>,
    libraries: &Path,
) -> Result<String> {
    let expanded = expand_tokens(raw, tokens)?;

    if let Some(coordinate) = expanded.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let path = libraries.join(MavenCoordinate::parse(coordinate)?.path());
        return path
            .into_os_string()
            .into_string()
            .map_err(|_| ModloaderError::Invalid("non-Unicode processor path".into()));
    }

    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tokens_without_reparsing_windows_paths() {
        let tokens = HashMap::from([("ROOT".into(), "C:\\Games\\My pack".into())]);
        assert_eq!(
            expand_tokens("--root={ROOT}", &tokens).unwrap(),
            "--root=C:\\Games\\My pack"
        );
        assert_eq!(
            expand_tokens("'literal {ROOT}'", &tokens).unwrap(),
            "literal {ROOT}"
        );
    }

    #[test]
    fn unknown_tokens_fail_instead_of_becoming_empty() {
        assert!(expand_tokens("{MISSING}", &HashMap::new()).is_err());
    }
}
