use std::path::Path;

use crate::instance::download::transfer::Cancel;
use crate::modloader::{
    self,
    components::Components,
    error::{ModloaderError, Result},
    repository::Repository,
};

mod package;
mod releases;

pub use releases::{get_releases, import_archive, register, CleanroomRelease};

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

    let releases = modloader::cleanroom::get_releases(context.repo).await?;
    let release = releases
        .iter()
        .find(|v| v.version == request.loader_version)
        .ok_or_else(|| {
            ModloaderError::Invalid(
                "Cleanroom release is not registered for this Minecraft version".into(),
            )
        })?;

    let game_components = package::prepare(
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
