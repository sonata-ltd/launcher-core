use std::path::Path;

use crate::modloader::{
    self,
    components::Components,
    error::{ModloaderError, Result},
    repository::Repository,
    transfer::Cancel,
};

pub(crate) struct PrepareRequest {
    pub(crate) loader_version: String,
}

pub struct PrepareContext<'a> {
    pub(crate) repo: &'a Repository,
    pub(crate) maven_dir: &'a Path,
    pub(crate) work_dir: &'a Path,
    pub(crate) cancel: &'a Cancel,
}

pub(crate) async fn prepare<'a>(
    request: PrepareRequest,
    context: PrepareContext<'a>,
) -> Result<Components> {
    context.cancel.check()?;

    let releases = modloader::cleanroom::releases(context.repo).await?;
    let release = releases
        .iter()
        .find(|v| v.version == request.loader_version)
        .ok_or_else(|| {
            ModloaderError::Invalid(
                "Cleanroom release is not registered for this Minecraft version".into(),
            )
        })?;

    let game_components = modloader::cleanroom::prepare(
        context.repo,
        release,
        context.work_dir,
        context.maven_dir,
        context.cancel,
    )
    .await?;

    context.cancel.check()?;

    Ok(game_components)
}
