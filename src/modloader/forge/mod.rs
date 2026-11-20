use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;

use crate::data::registry::operation::handle::OperationHandle;
use crate::data::registry::operation::message::stage::{OperationStage, StageResult};
use crate::data::registry::operation::message::status::Outcome;
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
use crate::utils::download::transfer::Cancel;
use crate::utils::files;
use crate::utils::maven::MavenCoordinate;

mod install;
mod model;
mod output;
mod processor;
mod runtime;
mod tokens;

#[cfg(test)]
mod tests;

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
    pub op: &'a OperationHandle,
    pub cancel: &'a Cancel,
}

pub struct PreparedForge {
    pub components: components::Components,
    pub pending_processors: Option<Prepared>,
}

pub fn install_sequence() -> [OperationStage; 1] {
    [OperationStage::RunModloaderProcessors]
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

    // The generation Maven directory is launcher-owned below its generation.
    let maven_dir = context.maven_dir.to_path_buf();
    async_std::task::spawn_blocking(move || {
        files::directory::Directory::open_child_of(&maven_dir, true)
    })
    .await?;

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
            context.cancel,
        )
        .await?;

        let pending_processors = if prepared.has_client_processors() {
            Some(prepared)
        } else {
            context.op.skip_stage(
                OperationStage::RunModloaderProcessors,
                Some("no client processors".into()),
            );

            None
        };

        Ok(PreparedForge {
            components: game_components,
            pending_processors,
        })
    } else {
        if spec.kind == LoaderKind::NeoForge {
            return Err(ModloaderError::Unsupported(
                "NeoForge component entrypoint schema".into(),
            ));
        }

        components::apply(&mut game_components, &modloader_manifest_value)?;

        context.op.skip_stage(
            OperationStage::RunModloaderProcessors,
            Some("no client processors".into()),
        );

        Ok(PreparedForge {
            components: game_components,
            pending_processors: None,
        })
    }
}

pub struct FinalizeRequest<'a> {
    pub game_version: &'a str,
    pub prepared: Prepared,
    pub maven_dir: &'a Path,
}

pub struct FinalizeContext<'a> {
    pub modloader_java_runtimes: &'a [Arc<JavaRuntime>],
    pub op: &'a OperationHandle,
    pub cancel: &'a Cancel,
}

/// Unscoped compatibility entry point for externally retained Java. Core
/// installation uses explicit managed retention, never infers it from a path.
pub async fn finalize<'a>(
    request: FinalizeRequest<'a>,
    context: &FinalizeContext<'a>,
    game_components: &Components,
) -> Result<()> {
    finalize_impl(request, context, game_components, None).await
}

pub(crate) async fn finalize_retained<'a>(
    request: FinalizeRequest<'a>,
    context: &FinalizeContext<'a>,
    game_components: &Components,
    java_use: &crate::java::managed::process::JavaUse,
) -> Result<()> {
    finalize_impl(request, context, game_components, Some(java_use)).await
}

async fn finalize_impl<'a>(
    request: FinalizeRequest<'a>,
    context: &FinalizeContext<'a>,
    game_components: &Components,
    java_use: Option<&crate::java::managed::process::JavaUse>,
) -> Result<()> {
    context.cancel.check()?;

    let java = runtime::select_for_processors(
        &request.prepared.profile.processors,
        request.maven_dir,
        context.modloader_java_runtimes,
        context.cancel,
    )
    .await?;

    let stage_start_point = Instant::now();
    context
        .op
        .start_stage_indeterminable(OperationStage::RunModloaderProcessors);

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

    match java_use {
        Some(java_use) => {
            install::run_retained(
                request.prepared,
                request.maven_dir.to_path_buf(),
                request.maven_dir.join(client),
                PathBuf::from(&java.exec_path),
                context.cancel,
                context.op,
                Some(java_use),
            )
            .await?
        }
        None => {
            install::run(
                request.prepared,
                request.maven_dir.to_path_buf(),
                request.maven_dir.join(client),
                PathBuf::from(&java.exec_path),
                context.cancel,
                context.op,
            )
            .await?
        }
    }

    context.op.complete_stage(StageResult {
        status: Outcome::Completed,
        stage: OperationStage::RunModloaderProcessors,
        duration_secs: stage_start_point.elapsed().as_secs_f64(),
        error: None,
    });

    Ok(())
}
