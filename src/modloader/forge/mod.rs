use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;

use crate::data::registry::operation::handle::OperationHandle;
use crate::instance::download::transfer::Cancel;
use crate::java;
use crate::java::model::JavaRuntime;
use crate::modloader::components;
use crate::modloader::components::Components;
use crate::modloader::error::{ModloaderError, Result};
use crate::modloader::forge::install::Prepared;
use crate::modloader::model::MetadataPin;
use crate::modloader::repository::Repository;
use crate::modloader::{
    catalog::ModloaderCatalog,
    model::{LoaderKind, LoaderSpec},
};
use crate::utils::files;
use crate::utils::maven::MavenCoordinate;

mod install;
mod model;
mod processor;
mod tokens;

pub struct PrepareRequest {
    pub game_version: String,
    pub loader_kind: LoaderKind,
    pub loader_version: String,
}

pub struct PrepareContext<'a> {
    pub repo: &'a Repository,
    pub catalog: &'a ModloaderCatalog,
    pub maven_dir: &'a Path,
    pub work_dir: &'a Path,
    pub cancel: &'a Cancel,
}

pub struct PreparedForge {
    pub components: components::Components,
    pub installer: Option<Prepared>,
}

pub async fn prepare<'a>(
    request: PrepareRequest,
    context: &PrepareContext<'a>,
    mut game_components: Components,
) -> Result<PreparedForge> {
    context.cancel.check()?;

    if !matches!(
        request.loader_kind,
        LoaderKind::Forge | LoaderKind::NeoForge
    ) {
        return Err(ModloaderError::Invalid(
            "forge::prepare supports only Forge and NeoForge loaders".into(),
        ));
    }

    let spec = LoaderSpec {
        kind: request.loader_kind,
        version: Some(request.loader_version),
    };

    spec.validate(&request.game_version)?;

    let entry = context
        .catalog
        .versions(spec.kind, &request.game_version)
        .await?
        .into_iter()
        .find(|v| Some(&v.version) == spec.version.as_ref())
        .ok_or_else(|| {
            ModloaderError::Invalid("loader not compatible with current Minecraft version".into())
        })?;

    let url = entry
        .component_url
        .ok_or_else(|| ModloaderError::Invalid("missing component URL".into()))?;

    let modloader_manifest_bytes = context.repo.bytes(&url, entry.sha256.as_deref()).await?;
    let modloader_manifest_value: Value = serde_json::from_slice(&modloader_manifest_bytes)?;

    game_components.pins.push(MetadataPin {
        url,
        sha256: files::sha256(&modloader_manifest_bytes),
    });

    async_std::fs::create_dir_all(&context.maven_dir).await?;

    if modloader_manifest_value
        .get("mainClass")
        .and_then(Value::as_str)
        == Some("io.github.zekerzhayard.forgewrapper.installer.Main")
    {
        let installer_jar = install::installer(
            context.repo,
            &modloader_manifest_value,
            &context.work_dir.join("installer.jar"),
            context.cancel,
        )
        .await?;

        context.cancel.check()?;

        let prepared = install::prepare(
            installer_jar,
            context.work_dir.join("installer"),
            context.maven_dir.into(),
            &request.game_version,
            &mut game_components,
        )
        .await?;

        Ok(PreparedForge {
            components: game_components,
            installer: Some(prepared),
        })
    } else {
        if spec.kind == LoaderKind::NeoForge {
            return Err(ModloaderError::Unsupported(
                "NeoForge component entrypoint schema".into(),
            ));
        }

        components::apply(&mut game_components, &modloader_manifest_value)?;

        Ok(PreparedForge {
            components: game_components,
            installer: None,
        })
    }
}

pub struct FinalizeRequest<'a> {
    pub game_dir: &'a Path,
    pub game_version: &'a str,
    pub prepared: Prepared,
    pub maven_dir: &'a Path,
}

pub struct FinalizeContext<'a> {
    pub modloader_java_runtimes: &'a [Arc<JavaRuntime>],
    pub op: &'a OperationHandle,
    pub cancel: &'a Cancel,
}

pub async fn finalize<'a>(
    request: FinalizeRequest<'a>,
    context: &FinalizeContext<'a>,
    game_components: &Components,
) -> Result<()> {
    context.cancel.check()?;

    let java = context
        .modloader_java_runtimes
        .iter()
        .filter(|j| {
            j.arch
                .as_deref()
                .is_none_or(|a| java::select::normalize_arch(a) == java::select::host_arch())
        })
        .max_by_key(|j| j.major)
        .ok_or_else(|| {
            ModloaderError::Invalid("register a Java runtime for installer processors".into())
        })?;

    let client = game_components
        .manifest
        .get("mainJar")
        .and_then(|v| v.get("name"))
        .and_then(Value::as_str)
        .map(MavenCoordinate::parse)
        .transpose()?
        .map(|v| v.path())
        .unwrap_or(
            MavenCoordinate::parse(&format!(
                "com.mojang:minecraft:{}:client",
                request.game_version
            ))?
            .path(),
        );

    install::run(
        request.prepared,
        request.maven_dir.to_path_buf(),
        request.maven_dir.join(client),
        request.game_dir.to_path_buf(),
        PathBuf::from(&java.exec_path),
        context.cancel,
        context.op,
    )
    .await?;

    Ok(())
}
