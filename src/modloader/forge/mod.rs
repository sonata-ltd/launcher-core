use self::{model::ModernInstallProfile, processor::ProcessorCommand};
use super::{
    archive, components,
    error::{ModloaderError, Result},
    files,
    maven::MavenCoordinate,
    repository::Repository,
    transfer::{self, Cancel},
};
use crate::data::registry::operation::message::stage::OperationStage;
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub mod model;
pub mod processor;
pub mod tokens;

pub async fn installer(
    repo: &Repository,
    component: &Value,
    destination: &Path,
    cancel: &Cancel,
) -> Result<PathBuf> {
    let mut installers = Vec::new();

    for value in component
        .get("mavenFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(name) = value.get("name").and_then(Value::as_str) {
            if MavenCoordinate::parse(name)?.classifier.as_deref() == Some("installer") {
                installers.push(value);
            }
        }
    }

    if installers.len() != 1 {
        return Err(ModloaderError::Unsupported(
            "component has no unique installer artefact".into(),
        ));
    }

    let descriptor = installers[0]
        .pointer("/downloads/artifact")
        .ok_or_else(|| ModloaderError::Invalid("installer artifact missing".into()))?;
    let url = descriptor
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| ModloaderError::Invalid("installer URL missing".into()))?;
    let hash = descriptor
        .get("sha1")
        .and_then(Value::as_str)
        .ok_or_else(|| ModloaderError::Invalid("installer hash missing".into()))?;

    let _ = repo;

    transfer::download(url, destination, Some(hash), cancel).await?;
    Ok(destination.into())
}

pub struct Prepared {
    pub profile: ModernInstallProfile,
    pub installer: PathBuf,
    pub extracted: PathBuf,
}

pub async fn prepare(
    installer: PathBuf,
    extracted: PathBuf,
    maven: PathBuf,
    game_version: &str,
    base: &mut components::Components,
) -> Result<Prepared> {
    let input = installer.clone();
    let dir = extracted.clone();

    let value: Value = async_std::task::spawn_blocking(move || {
        archive::extract(&input, &dir, &[])?;
        Ok::<Value, ModloaderError>(serde_json::from_slice(&archive::read(
            &input,
            "install_profile.json",
            4 * 1024 * 1024,
        )?)?)
    })
    .await?;

    if value.get("install").is_some() || value.get("versionInfo").is_some() {
        return Err(ModloaderError::Unsupported(
            "
            legacy installer; use legacy Prism component"
                .into(),
        ));
    }

    let profile: ModernInstallProfile = serde_json::from_value(value)?;
    if profile.spec.is_some_and(|s| s > 1) || profile.minecraft != game_version {
        return Err(ModloaderError::Unsupported(
            "installer spec or Minecraft version mismatch".into(),
        ));
    }

    let input = installer.clone();
    let json = profile.json.clone();
    let mut patch: Value = async_std::task::spawn_blocking(move || {
        Ok::<Value, ModloaderError>(serde_json::from_slice(&archive::read(
            &input,
            &json,
            4 * 1024 * 1024,
        )?)?)
    })
    .await?;

    if patch
        .get("inheritsFrom")
        .and_then(Value::as_str)
        .is_some_and(|v| v != game_version)
    {
        return Err(ModloaderError::Invalid(
            "installer version targets another Minecraft version".into(),
        ));
    }

    // Modern game arguments extend the inherited Minecraft command. Legacy
    // minecraftArguments remains a complete template handled by components::apply.
    patch
        .as_object_mut()
        .ok_or_else(|| ModloaderError::Invalid("version profile is not object".into()))?
        .remove("inheritsFrom");
    components::apply(base, &patch)?;
    base.maven_files.extend(profile.libraries.clone());

    let embedded = extracted.join("maven");
    if embedded.is_dir() {
        copy_tree(embedded, maven).await?;
    }

    Ok(Prepared {
        profile,
        installer,
        extracted,
    })
}

async fn copy_tree(source: PathBuf, destination: PathBuf) -> Result<()> {
    async_std::task::spawn_blocking(move || {
        let mut stack = vec![(source, destination)];

        while let Some((source, target)) = stack.pop() {
            std::fs::create_dir_all(&target)?;

            for item in std::fs::read_dir(source)? {
                let item = item?;
                let dest = target.join(item.file_name());
                let kind = item.file_type()?;

                if kind.is_symlink() {
                    return Err(ModloaderError::Invalid("embedded Maven symlink".into()));
                }

                if kind.is_dir() {
                    stack.push((item.path(), dest));
                } else {
                    std::fs::copy(item.path(), dest)?;
                }
            }
        }
        Ok(())
    })
    .await
}

pub async fn run(
    prepared: Prepared,
    libraries: PathBuf,
    minecraft: PathBuf,
    game_root: PathBuf,
    java: PathBuf,
    cancel: &Cancel,
    op: &crate::data::registry::operation::handle::OperationHandle,
) -> Result<()> {
    let mut tokens = HashMap::from([
        ("SIDE".into(), "client".into()),
        (
            "MINECRAFT_VERSION".into(),
            prepared.profile.minecraft.clone(),
        ),
        (
            "MINECRAFT_JAR".into(),
            minecraft.to_string_lossy().into_owned(),
        ),
        ("ROOT".into(), game_root.to_string_lossy().into_owned()),
        (
            "INSTALLER".into(),
            prepared.installer.to_string_lossy().into_owned(),
        ),
        (
            "LIBRARY_DIR".into(),
            libraries.to_string_lossy().into_owned(),
        ),
    ]);

    for (key, side) in &prepared.profile.data {
        let Some(raw) = &side.client else {
            continue;
        };

        let value = if raw.starts_with('[') {
            tokens::resolve_argument(raw, &tokens, &libraries)?
        } else if raw.starts_with('\'') {
            tokens::expand_tokens(raw, &tokens)?
        } else {
            let path = PathBuf::from(raw.trim_start_matches('/'));

            files::relative(&path)?;

            let path = files::child(&prepared.extracted, &path)?;

            if !path.is_file() {
                return Err(ModloaderError::Invalid(
                    "installer data file missing".into(),
                ));
            }

            path.to_string_lossy().into_owned()
        };

        tokens.insert(key.clone(), value);
    }

    let total = prepared
        .profile
        .processors
        .iter()
        .filter(|step| step.applies_to_client())
        .count();

    let mut completed = 0;
    op.update_stage(OperationStage::CreateStructure, 0, total, None);

    for (index, step) in prepared.profile.processors.into_iter().enumerate() {
        if !step.applies_to_client() {
            continue;
        }

        cancel.check()?;

        let jar = libraries.join(MavenCoordinate::parse(&step.jar)?.path());
        let input = jar.clone();
        let main_class =
            async_std::task::spawn_blocking(move || archive::main_class(&input)).await?;
        let mut classpath = vec![jar];

        for item in step.classpath {
            classpath.push(libraries.join(MavenCoordinate::parse(&item)?.path()));
        }

        let args = step
            .args
            .iter()
            .map(|arg| tokens::resolve_argument(&arg, &tokens, &libraries))
            .collect::<Result<Vec<_>>>()?;

        let mut outputs = Vec::new();
        for (path, hash) in step.outputs {
            let path = PathBuf::from(tokens::resolve_argument(&path, &tokens, &libraries)?);

            if !path.starts_with(&libraries) {
                return Err(ModloaderError::Invalid(
                    "processor output escapes installation Maven root".into(),
                ));
            }

            if let Some(parent) = path.parent() {
                async_std::fs::create_dir_all(parent).await?;
            }

            outputs.push((path, tokens::expand_tokens(&hash, &tokens)?));
        }

        processor::run(
            ProcessorCommand {
                java: java.clone(),
                main_class,
                classpath,
                args,
                cwd: prepared.extracted.clone(),
                log: prepared.extracted.join(format!("processor-{index}.log")),
                outputs,
            },
            &cancel.0,
        )
        .await?;

        completed += 1;
        op.update_stage(OperationStage::CreateStructure, completed, total, None);
    }

    Ok(())
}
