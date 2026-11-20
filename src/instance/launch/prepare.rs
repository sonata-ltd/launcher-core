use std::sync::Arc;

use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::{
        download::{libs::linked_libs_at, manifest::load_or_fetch},
        error::InstanceError,
        model::InstanceRecord,
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
    let libs = linked_libs_at(rec.id, db, paths.libraries()).await?;
    if libs.is_empty() {
        return Err(InstanceError::NotInstalled(rec.id));
    }
    let manifest = load_or_fetch(&rec.manifest_url, paths.meta()).await?;
    Ok((manifest, libs))
}
