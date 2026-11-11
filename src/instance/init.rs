use std::{path::PathBuf, sync::Arc, time::Instant};

use async_std::fs;

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
        download::{
            assets::AssetsData,
            libs::LibsData,
            manifest::{fetch_and_cache_manifest, get_assets_manifest},
        },
        error::InstanceError,
        model::{InstanceId, InstanceRecord},
        paths::InstancePaths,
        Result,
    },
    java::{model::JavaRuntime, select::JavaRequirement},
    modloader::{
        catalog::ModloaderCatalog,
        model::{InstalledProfile, LoaderSpec},
        transfer::Cancel,
    },
    version::provider::MetaProvider,
};

pub struct InstallRequest {
    instance_id: InstanceId,
    instance_dir_name: String,
    game_version: String,
    manifest_url: String,
    provider: MetaProvider,
    loader: LoaderSpec,
}

pub struct InstallContext<'a> {
    paths: &'a LauncherPaths,
    db: Arc<Database>,
    catalog: ModloaderCatalog,
    // Java runtimes that can be used during modloader installation.
    modloader_java_runtimes: &'a [Arc<JavaRuntime>],
    op: &'a OperationHandle,
    cancel: &'a Cancel,
}

pub async fn install<'a>(
    request: &InstallRequest,
    context: &InstallContext<'a>,
) -> Result<InstalledProfile> {
    let instance_paths = InstancePaths::resolve(context.paths, &request.instance_dir_name);

    fs::create_dir_all(instance_paths.root())
        .await
        .map_err(|e| InstanceError::DirCreationFailed(e.to_string()))?;

    // Stage 1 - Get Minecraft version manifest
    context.op.start_stage(OperationStage::FetchManifest);
    let started = Instant::now();

    let (version_manifest, _) =
        fetch_and_cache_manifest(&request.manifest_url, context.paths.meta())
            .await
            .map_err(|e| {
                InstanceError::CreationFailed(format!("failed to download version manifest: {e}"))
            })?;

    let java_requirement = JavaRequirement::from_manifest(&version_manifest);

    context.op.complete_stage(StageResult {
        status: Outcome::Completed,
        stage: OperationStage::FetchManifest,
        duration_secs: started.elapsed().as_secs_f64(),
        error: None,
    });

    // Stage 2 - Sync & download all libs needed by this version
    LibsData::sync(
        request.instance_id,
        &version_manifest,
        context.paths,
        request.provider,
        Arc::clone(&context.db),
        context.op,
    )
    .await
    .map_err(|e| InstanceError::CreationFailed(format!("failed to sync libs: {e}")))?;

    // Get version assets manifest
    let (assets_manifest, _) =
        get_assets_manifest(&version_manifest, context.paths.assets().join("indexes"))
            .await
            .map_err(|e| {
                InstanceError::CreationFailed(format!("failed to download assets manifest: {e}"))
            })?;

    // Stage 3 - Sync & download all assets needed by this version
    AssetsData::sync(
        &assets_manifest,
        context.paths,
        Arc::clone(&context.db),
        context.op,
    )
    .await
    .map_err(|e| InstanceError::CreationFailed(format!("failed to sync assets: {e}")))?;

    Ok(java_requirement)
}
