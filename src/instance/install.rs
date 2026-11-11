use std::{path::PathBuf, sync::Arc, time::Instant};

#[cfg(test)]
mod tests;

use crate::{
    data::{
        db::Database,
        layout::LauncherPaths,
        registry::operation::{
            handle::OperationHandle,
            message::{
                stage::{OperationStage, StageResult},
                status::Outcome,
            },
        },
    },
    instance::{
        self,
        download::{assets::AssetsData, manifest::get_assets_manifest},
        model::InstanceId,
        workspace::InstallWorkspace,
    },
    java::model::JavaRuntime,
    modloader::{
        self,
        artifacts::{self, Wanted},
        catalog::ModloaderCatalog,
        components,
        error::{ModloaderError, Result},
        loaders::{cleanroom, fabric_quilt, forge},
        model::{InstalledProfile, JavaPolicy, LoaderKind, LoaderSpec, MetadataPin},
        transfer::Cancel,
    },
    version::provider::MetaProvider,
};

fn complete(op: &OperationHandle, stage: OperationStage, started: Instant) {
    op.complete_stage(StageResult {
        stage,
        status: Outcome::Completed,
        duration_secs: started.elapsed().as_secs_f64(),
        error: None,
    });
}

pub(crate) struct InstallRequest {
    pub(crate) instance_id: InstanceId,
    pub(crate) instance_dir_name: String,
    pub(crate) game_version: String,
    pub(crate) manifest_url: String,
    pub(crate) meta_provider: MetaProvider,
    pub(crate) loader: LoaderSpec,
}

pub(crate) struct InstallContext<'a> {
    pub(crate) paths: &'a LauncherPaths,
    pub(crate) db: Arc<Database>,
    pub(crate) catalog: &'a ModloaderCatalog,
    // Java runtimes that can be used during modloader installation.
    pub(crate) modloader_java_runtimes: &'a [Arc<JavaRuntime>],
    pub(crate) op: &'a OperationHandle,
    pub(crate) cancel: &'a Cancel,
}

pub(crate) async fn execute<'a>(
    request: &InstallRequest,
    context: &InstallContext<'a>,
) -> Result<InstalledProfile> {
    context.cancel.check()?;
    request.loader.validate(&request.game_version)?;

    let workspace = InstallWorkspace::create(context.paths, request.instance_id).await?;

    let result = async {
        let unvalidated_profile = install(request, context, &workspace).await?;

        context.cancel.check()?;

        let started = Instant::now();
        context.op.start_stage(OperationStage::VerifyFiles);

        let artifacts = artifacts::finish(
            unvalidated_profile.wanted,
            workspace.maven_dir().to_path_buf(),
            context.op,
        )
        .await?;

        let profile = InstalledProfile {
            format_version: unvalidated_profile.format_version,
            minecraft: unvalidated_profile.minecraft,
            loader: unvalidated_profile.loader,
            manifest: unvalidated_profile.manifest,
            java: unvalidated_profile.java,
            libraries_root: unvalidated_profile.libraries_root,
            artifacts,
            metadata: unvalidated_profile.metadata,
        };

        context.cancel.check()?;

        modloader::storage::publish(&context.db, request.instance_id, &profile)
            .await
            .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

        complete(context.op, OperationStage::VerifyFiles, started);

        Ok::<InstalledProfile, ModloaderError>(profile)
    }
    .await;

    match result {
        Ok(profile) => {
            if let Err(cleanup_error) = workspace.cleanup_work().await {
                tracing::warn!(
                    error = %cleanup_error,
                    "Failed to remove installation work files"
                );
            }

            Ok(profile)
        }
        Err(install_error) => {
            if let Err(cleanup_error) = workspace.cleanup_failed().await {
                tracing::warn!(
                    error = %cleanup_error,
                    "Failed to remove unfinished installation work files"
                );
            }

            Err(install_error)
        }
    }
}

pub(crate) struct UnvalidatedProfile {
    pub format_version: u32,
    pub minecraft: String,
    pub loader: LoaderSpec,
    pub manifest: serde_json::Value,
    pub java: JavaPolicy,
    pub libraries_root: PathBuf,
    pub wanted: Vec<Wanted>,
    pub metadata: Vec<MetadataPin>,
}

async fn install<'a>(
    request: &InstallRequest,
    context: &InstallContext<'a>,
    workspace: &InstallWorkspace,
) -> Result<UnvalidatedProfile> {
    context.cancel.check()?;

    let loader_spec = request.loader.clone();

    let previous_installation = modloader::storage::load(&context.db, request.instance_id)
        .await
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    let pinned_repo = context.catalog.repository.pinned(
        previous_installation
            .as_ref()
            .filter(|p| p.loader == loader_spec && p.minecraft == request.game_version)
            .map(|p| p.metadata.as_slice())
            .unwrap_or(&[]),
    );

    let repo = &pinned_repo;
    let blobs_path = context.paths.libraries().join("blobs");

    let started = Instant::now();
    context.op.start_stage(OperationStage::FetchManifest);

    let mut game_components = if loader_spec.kind == LoaderKind::Cleanroom {
        cleanroom::prepare(
            cleanroom::PrepareRequest {
                loader_version: loader_spec
                    .version
                    .clone()
                    .ok_or_else(|| ModloaderError::Invalid("missing loader version".into()))?,
            },
            cleanroom::PrepareContext {
                repo,
                maven_dir: workspace.maven_dir(),
                work_dir: workspace.work_dir(),
                cancel: context.cancel,
            },
        )
        .await?
    } else {
        instance::prepare::prepare(
            instance::prepare::PrepareRequest {
                game_version: &request.game_version,
                manifest_url: &request.manifest_url,
                meta_provider: request.meta_provider,
            },
            instance::prepare::PrepareContext {
                repo: repo,
                cancel: context.cancel,
            },
        )
        .await?
    };

    // Forge installation requires post install steps.
    let mut forge_installer = None;

    if matches!(loader_spec.kind, LoaderKind::Fabric | LoaderKind::Quilt) {
        game_components = fabric_quilt::prepare(
            fabric_quilt::PrepareRequest {
                game_version: &request.game_version,
                loader_kind: loader_spec.kind,
                loader_version: loader_spec
                    .version
                    .clone()
                    .ok_or_else(|| ModloaderError::Invalid("missing loader version".into()))?,
            },
            &fabric_quilt::PrepareContext {
                repo,
                cancel: context.cancel,
            },
            game_components,
        )
        .await?;
    } else if matches!(loader_spec.kind, LoaderKind::Forge | LoaderKind::NeoForge) {
        let prepared = forge::prepare(
            forge::PrepareRequest {
                game_version: request.game_version.clone(),
                loader_kind: loader_spec.kind,
                loader_version: loader_spec
                    .version
                    .clone()
                    .ok_or_else(|| ModloaderError::Invalid("missing loader version".into()))?,
            },
            &forge::PrepareContext {
                repo,
                catalog: context.catalog,
                maven_dir: workspace.maven_dir(),
                work_dir: workspace.work_dir(),
                cancel: context.cancel,
            },
            game_components,
        )
        .await?;

        game_components = prepared.components;
        forge_installer = prepared.installer;
    }

    let java_runtime_policy = components::policy(&game_components.manifest)?;

    complete(context.op, OperationStage::FetchManifest, started);

    let wanted = artifacts::parse(&mut game_components.manifest, &game_components.maven_files)?;

    let started = Instant::now();
    context.op.start_stage(OperationStage::DownloadLibs);

    artifacts::hydrate(
        &wanted,
        workspace.maven_dir(),
        &blobs_path,
        context.cancel,
        context.op,
    )
    .await?;

    complete(context.op, OperationStage::DownloadLibs, started);

    let started = Instant::now();
    context.op.start_stage(OperationStage::CreateStructure);

    if let Some(prepared) = forge_installer {
        modloader::loaders::forge::finalize(
            forge::FinalizeRequest {
                game_dir: &context
                    .paths
                    .instances()
                    .join(request.instance_dir_name.clone()),
                game_version: &request.game_version,
                prepared,
                maven_dir: workspace.maven_dir(),
            },
            &forge::FinalizeContext {
                modloader_java_runtimes: context.modloader_java_runtimes,
                op: context.op,
                cancel: context.cancel,
            },
            &game_components,
        )
        .await?;
    }

    complete(context.op, OperationStage::CreateStructure, started);
    context.cancel.check()?;

    let (assets, _) = get_assets_manifest(
        &game_components.manifest,
        context.paths.assets().join("indexes"),
    )
    .await
    .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    AssetsData::sync(&assets, context.paths, Arc::clone(&context.db), context.op)
        .await
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    let profile = UnvalidatedProfile {
        format_version: 1,
        minecraft: request.game_version.clone(),
        loader: loader_spec,
        manifest: game_components.manifest,
        java: java_runtime_policy,
        libraries_root: workspace.relative_maven_dir().to_path_buf(),
        wanted,
        metadata: game_components.pins,
    };

    Ok(profile)
}
