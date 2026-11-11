use crate::utils::{files, maven::MavenCoordinate, url};

use super::{
    error::{ModloaderError, Result},
    model::MetadataPin,
    repository::Repository,
};

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct Components {
    pub manifest: Value,
    pub pins: Vec<MetadataPin>,
    pub maven_files: Vec<Value>,
}

fn strings(value: Option<&Value>) -> Result<Vec<Value>> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(v)) if v.iter().all(Value::is_string) => Ok(v.clone()),
        Some(Value::String(v)) => shlex::split(v)
            .map(|v| v.into_iter().map(Value::String).collect())
            .ok_or_else(|| ModloaderError::Invalid("invalid argument quoting".into())),
        _ => Err(ModloaderError::Invalid(
            "invalid component arguments".into(),
        )),
    }
}

pub fn apply(target: &mut Components, component: &Value) -> Result<()> {
    if !component.is_object() {
        return Err(ModloaderError::Invalid("component is not an object".into()));
    }

    for key in [
        "jarMods",
        "+jarMods",
        "mods",
        "+mods",
        "-libraries",
        "+agents",
    ] {
        if component
            .get(key)
            .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
        {
            return Err(ModloaderError::Unsupported(format!(
                "component operation: {key}"
            )));
        }
    }

    if !target.manifest.is_object() {
        target.manifest = json!({
            "libraries": [],
            "arguments": {
                "game": [],
                "jvm": []
            }
        });
    }

    for field in [
        "mainClass",
        "mainJar",
        "assetIndex",
        "downloads",
        "compatibleJavaMajors",
        "javaVersion",
        "logging",
    ] {
        if let Some(value) = component.get(field) {
            target.manifest[field] = value.clone();
        }
    }

    if let Some(arguments) = component.get("arguments") {
        for side in ["game", "jvm"] {
            if let Some(array) = arguments.get(side) {
                let array = array
                    .as_array()
                    .ok_or_else(|| ModloaderError::Invalid("arguments must be arrays".into()))?;

                target.manifest["arguments"][side]
                    .as_array_mut()
                    .unwrap()
                    .extend(array.clone());
            }
        }
    } else if let Some(legacy) = component.get("minecraftArguments") {
        target.manifest["arguments"]["game"] = Value::Array(strings(Some(legacy))?);
    }

    let extra = strings(component.get("+jvmArgs"))?;
    target.manifest["arguments"]["jvm"]
        .as_array_mut()
        .unwrap()
        .extend(extra);

    for tweaker in strings(component.get("+tweakers"))? {
        target.manifest["arguments"]["game"]
            .as_array_mut()
            .unwrap()
            .extend([Value::String("--tweakClass".into()), tweaker]);
    }

    if let Some(traits) = component.get("+traits").and_then(Value::as_array) {
        let mut all = target
            .manifest
            .get("+traits")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        all.extend(traits.clone());
        target.manifest["+traits"] = Value::Array(all);
    }

    let mut wanted = component
        .get("libraries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    if let Some(agents) = component.get("agents").and_then(Value::as_array) {
        for agent in agents {
            let coordinate = agent
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ModloaderError::Invalid("agent has no name".into()))?;
            let path = MavenCoordinate::parse(coordinate)?.path();

            target.manifest["arguments"]["jvm"]
                .as_array_mut()
                .unwrap()
                .push(Value::String(format!(
                    "-javaagent:${{library_directory}}/{}",
                    path.to_string_lossy().replace('\\', "/")
                )));

            wanted.push(agent.clone());
        }
    }

    let libs = target.manifest["libraries"].as_array_mut().unwrap();
    let mut replaced = HashSet::new();

    for library in wanted {
        let name = library
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ModloaderError::Invalid("library has no name".into()))?;
        let key = MavenCoordinate::parse(name)?.conflict_key();

        if replaced.insert(key.clone()) {
            let mut retained = Vec::new();

            for old in libs.drain(..) {
                let same = old
                    .get("name")
                    .and_then(Value::as_str)
                    .map(MavenCoordinate::parse)
                    .transpose()?
                    .is_some_and(|v| v.conflict_key() == key);
                if !same {
                    retained.push(old);
                }
            }

            *libs = retained;
        }

        libs.push(library);
    }

    if let Some(files) = component.get("mavenFiles").and_then(Value::as_array) {
        target.maven_files.extend(files.clone());
    }

    Ok(())
}

pub async fn prism(
    repo: &Repository,
    game_version: &str,
    base_url: &str,
    locals: HashMap<String, Value>,
    roots: Vec<(String, String)>,
) -> Result<Components> {
    enum Visit {
        Enter(String, String),
        Exit(String, Value),
    }

    let mut stack = Vec::new();
    let mut states = HashMap::<String, (String, bool)>::new();
    let mut result = Components::default();
    let selected: HashMap<String, String> = roots.iter().cloned().collect();

    for (uid, version) in roots.into_iter().rev() {
        stack.push(Visit::Enter(uid, version));
    }

    while let Some(item) = stack.pop() {
        match item {
            Visit::Enter(uid, requested) => {
                if let Some((selected, finished)) = states.get(&uid) {
                    if !finished {
                        return Err(ModloaderError::Invalid(format!(
                            "component dependency cycle: {uid}"
                        )));
                    }
                    if selected != &requested {
                        return Err(ModloaderError::Invalid(format!(
                            "component version conflict: {uid}"
                        )));
                    }
                    continue;
                }

                if states.len() >= 128 {
                    return Err(ModloaderError::Invalid("too many components".into()));
                }

                let component = if let Some(local) = locals.get(&uid) {
                    local.clone()
                } else {
                    let url =
                        url::append_segments(base_url, &[&uid, &format!("{requested}.json")])?;
                    let bytes = repo.bytes(&url, None).await?;
                    result.pins.push(MetadataPin {
                        url,
                        sha256: files::sha256(&bytes),
                    });
                    serde_json::from_slice(&bytes)?
                };

                if component.get("uid").and_then(Value::as_str) != Some(uid.as_str())
                    || component.get("version").and_then(Value::as_str) != Some(requested.as_str())
                {
                    return Err(ModloaderError::Invalid(
                        "component identity mismatch".into(),
                    ));
                }

                states.insert(uid.clone(), (requested, true));
                states.get_mut(&uid).unwrap().1 = false;
                stack.push(Visit::Exit(uid, component.clone()));

                let requires = component
                    .get("requires")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();

                for req in requires.into_iter().rev() {
                    let id = req
                        .get("uid")
                        .and_then(Value::as_str)
                        .ok_or_else(|| ModloaderError::Invalid("requirement has no uid".into()))?;
                    let exact = req.get("equals").and_then(Value::as_str);
                    let suggested = req.get("suggests").and_then(Value::as_str);
                    let chosen = if let Some(exact) = exact {
                        exact.to_owned()
                    } else if let Some(local) = locals
                        .get(id)
                        .and_then(|v| v.get("version"))
                        .and_then(Value::as_str)
                    {
                        local.into()
                    } else if let Some(v) = selected.get(id) {
                        v.clone()
                    } else if let Some((v, _)) = states.get(id) {
                        v.clone()
                    } else if id == "net.minecraft" {
                        game_version.into()
                    } else {
                        suggested
                            .ok_or_else(|| {
                                ModloaderError::Unsupported(format!("unversioned component: {id}"))
                            })?
                            .into()
                    };
                    stack.push(Visit::Enter(id.into(), chosen));
                }
            }

            Visit::Exit(uid, component) => {
                apply(&mut result, &component)?;
                states.get_mut(&uid).unwrap().1 = true;
            }
        }
    }

    result.manifest["id"] = Value::String(game_version.into());
    Ok(result)
}

pub fn policy(manifest: &Value) -> Result<super::model::JavaPolicy> {
    let req = crate::java::select::JavaRequirement::from_manifest(manifest);
    if req.majors.is_empty() {
        return Err(ModloaderError::Unsupported(
            "profile has no Java requirement".into(),
        ));
    }

    Ok(super::model::JavaPolicy {
        majors: req.majors,
        allow_newer: false,
    })
}

pub fn validate_unique_components(values: &[Value]) -> Result<()> {
    let mut seen = HashSet::new();

    for v in values {
        if !seen.insert(
            v.get("uid")
                .and_then(Value::as_str)
                .ok_or_else(|| ModloaderError::Invalid("missing component uid".into()))?,
        ) {
            return Err(ModloaderError::Invalid("duplicate component uid".into()));
        }
    }

    Ok(())
}
