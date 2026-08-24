use std::{sync::Arc, time::Instant};

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
        model::InstanceRecord,
        paths::InstancePaths,
        Result,
    },
    version::provider::MetaProvider,
};

pub async fn install(
    rec: &InstanceRecord,
    provider: MetaProvider,
    paths: &LauncherPaths,
    db: Arc<Database>,
    op: &OperationHandle,
) -> Result<()> {
    let instance_paths = InstancePaths::resolve(paths, &rec.dir);

    fs::create_dir_all(instance_paths.root())
        .await
        .map_err(|e| InstanceError::DirCreationFailed(e.to_string()))?;

    // Stage 1 - Get Minecraft version manifest
    op.start_stage(OperationStage::FetchManifest);
    let started = Instant::now();

    let (version_manifest, _) = fetch_and_cache_manifest(&rec.manifest_url, paths.meta())
        .await
        .map_err(|e| {
            InstanceError::CreationFailed(format!("failed to download version manifest: {e}"))
        })?;

    op.complete_stage(StageResult {
        status: Outcome::Completed,
        stage: OperationStage::FetchManifest,
        duration_secs: started.elapsed().as_secs_f64(),
        error: None,
    });

    // Stage 2 - Sync & download all libs needed by this version
    LibsData::sync(
        rec.id,
        &version_manifest,
        paths,
        provider,
        Arc::clone(&db),
        op,
    )
    .await
    .map_err(|e| InstanceError::CreationFailed(format!("failed to sync libs: {e}")))?;

    // Get version assets manifest
    let (assets_manifest, _) =
        get_assets_manifest(&version_manifest, paths.assets().join("indexes"))
            .await
            .map_err(|e| {
                InstanceError::CreationFailed(format!("failed to download assets manifest: {e}"))
            })?;

    // Stage 3 - Sync & download all assets needed by this version
    AssetsData::sync(&assets_manifest, paths, db, op)
        .await
        .map_err(|e| InstanceError::CreationFailed(format!("failed to sync assets: {e}")))?;

    Ok(())
}
