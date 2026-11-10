use super::{
    error::{ModloaderError, Result},
    maven::MavenCoordinate,
};
use serde_json::{json, Value};

fn args(profile: &Value, side: &str) -> Result<Vec<Value>> {
    if let Some(values) = profile.pointer(&format!("/arguments/{side}")) {
        return values
            .as_array()
            .cloned()
            .ok_or_else(|| ModloaderError::Invalid(format!("arguments.{side} must be an array")));
    }

    if side == "game" {
        if let Some(raw) = profile.get("minecraftArguments").and_then(Value::as_str) {
            return shlex::split(raw)
                .map(|args| args.into_iter().map(Value::String).collect())
                .ok_or_else(|| ModloaderError::Invalid("invalid legacy argument quoting".into()));
        }
    }

    Ok(Vec::new())
}

pub fn merge_official(base: &Value, patch: &Value, mc: &str) -> Result<Value> {
    let mut out = base
        .as_object()
        .cloned()
        .ok_or_else(|| ModloaderError::Invalid("base profile is not an object".into()))?;

    let patch_obj = patch
        .as_object()
        .ok_or_else(|| ModloaderError::Invalid("loader profile is not an object".into()))?;

    if patch_obj.get("inheritsFrom").and_then(Value::as_str) != Some(mc) {
        return Err(ModloaderError::Invalid(
            "inheritsFrom does not match Minecraft".into(),
        ));
    }

    let mut libraries = base
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if let Some(extra) = patch.get("libraries") {
        let extra = extra
            .as_array()
            .ok_or_else(|| ModloaderError::Invalid("libraries must be an array".into()))?;
        let key = |library: &Value| -> Result<_> {
            let name = library
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ModloaderError::Invalid("library has no coordinate".into()))?;
            Ok(MavenCoordinate::parse(name)?.conflict_key())
        };
        let mut replacements = std::collections::HashMap::new();
        let mut order = Vec::new();
        for library in extra {
            let key = key(library)?;
            if !replacements.contains_key(&key) {
                order.push(key.clone());
            }
            replacements
                .entry(key)
                .or_insert_with(Vec::new)
                .push(library.clone());
        }
        let mut merged = Vec::new();
        let mut replaced = std::collections::HashSet::new();
        for library in libraries {
            let key = key(&library)?;
            if let Some(group) = replacements.get(&key) {
                if replaced.insert(key) {
                    merged.extend(group.clone());
                }
            } else {
                merged.push(library);
            }
        }
        for key in order {
            if replaced.insert(key.clone()) {
                merged.extend(replacements.remove(&key).unwrap());
            }
        }
        libraries = merged;
    }

    let mut game = args(base, "game")?;
    game.extend(args(patch, "game")?);

    let mut jvm = args(base, "jvm")?;
    jvm.extend(args(patch, "jvm")?);

    for key in ["id", "mainClass", "type"] {
        if let Some(value) = patch_obj.get(key) {
            out.insert(key.into(), value.clone());
        }
    }

    out.insert("libraries".into(), Value::Array(libraries));
    out.insert("arguments".into(), json!({ "game": game, "jvm": jvm }));
    out.remove("minecraftArguments");
    out.remove("inheritsFrom");
    out.remove("requires");

    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_base_assets_and_appends_loader_arguments() {
        let base = json!({
            "assetIndex": {"id": "test"},
            "minecraftArguments": "--username ${auth_player_name}",
            "libraries": [{"name": "org.test:lib:1"}]
        });
        let patch = json!({
            "inheritsFrom": "1.20.1", "mainClass": "example.Loader",
            "arguments": {"game": ["--extra", "yes"]},
            "libraries": [{"name": "org.test:lib:2"}]
        });
        let result = merge_official(&base, &patch, "1.20.1").unwrap();
        assert_eq!(result["assetIndex"]["id"], "test");
        assert_eq!(result["libraries"].as_array().unwrap().len(), 1);
        assert_eq!(result["libraries"][0]["name"], "org.test:lib:2");
        assert_eq!(result["arguments"]["game"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn rejects_other_minecraft_version() {
        assert!(merge_official(&json!({}), &json!({"inheritsFrom": "1.19.2"}), "1.20.1").is_err());
    }
}
