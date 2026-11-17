use std::{path::PathBuf, sync::Arc};

use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::{
        download::{
            libs::linked_libs,
            manifest::{asset_index_id, load_or_fetch},
        },
        error::InstanceError,
        launch::{args::ArgType, LaunchInfo, LaunchInfoBuilder},
        model::InstanceRecord,
        paths::InstancePaths,
        settings::EffectiveSettings,
        Result,
    },
    java::model::JavaRuntime,
};

pub(crate) async fn prepare_command(
    rec: &InstanceRecord,
    settings: &EffectiveSettings,
    java: &JavaRuntime,
    paths: &LauncherPaths,
    db: Arc<Database>,
) -> Result<super::resolved::LaunchCommand> {
    let (manifest, libs) = legacy_inputs(rec, paths, db).await?;
    super::resolved::prepare_legacy(rec, settings, java, paths, manifest, libs).await
}

async fn legacy_inputs(
    rec: &InstanceRecord,
    paths: &LauncherPaths,
    db: Arc<Database>,
) -> Result<(
    serde_json::Value,
    Vec<crate::instance::download::libs::LibInfo>,
)> {
    let libs = linked_libs(rec.id, db).await?;
    if libs.is_empty() {
        return Err(InstanceError::NotInstalled(rec.id));
    }
    let manifest = load_or_fetch(&rec.manifest_url, paths.meta())
        .await
        .map_err(|error| {
            InstanceError::RunFailed(format!("failed to load version manifest: {error}"))
        })?;
    Ok((manifest, libs))
}

pub async fn prepare(
    rec: &InstanceRecord,
    settings: &EffectiveSettings,
    java: &JavaRuntime,
    paths: &LauncherPaths,
    db: Arc<Database>,
) -> Result<LaunchInfo> {
    let (manifest, libs) = legacy_inputs(rec, paths, db).await?;
    let asset_index = asset_index_id(&manifest)
        .map_err(|e| InstanceError::RunFailed(e.to_string()))?
        .to_string();

    let instance_paths = InstancePaths::resolve(paths, &rec.dir);

    let mut classpath = Vec::with_capacity(libs.len());
    let mut natives = Vec::new();

    for lib in &libs {
        let path = PathBuf::from(lib.path());

        if lib.is_native() {
            natives.push(path.clone());
        }

        classpath.push(path);
    }

    let mut builder = LaunchInfoBuilder::new();

    builder.set_manifest(manifest);
    builder.set_arg_value(ArgType::GameDir, instance_paths.root());
    builder.set_arg_value(ArgType::AssetsDir, paths.assets());
    builder.set_arg_value(ArgType::AssetIndex, &asset_index);
    builder.set_natives_dir(instance_paths.natives().clone());
    builder.set_memory(settings.memory_min, settings.memory_max);
    builder.set_jvm_args(settings.jvm_args.clone());
    builder.add_version(&rec.version);
    builder.add_cps(classpath);
    builder.add_natives(natives);
    builder.set_username("Player".to_string());

    Ok(builder
        .set_preferred_java_runtime(java.clone())
        .fill_defaults()
        .build()?)
}
