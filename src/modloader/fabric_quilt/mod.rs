use crate::{
    instance::download::transfer::Cancel,
    modloader::{
        self, components::Components, error::Result, model::LoaderKind, repository::Repository,
    },
};

mod metadata;

pub use metadata::versions;

pub struct PrepareRequest<'a> {
    pub game_version: &'a str,
    pub loader_kind: LoaderKind,
    pub loader_version: String,
}

pub struct PrepareContext<'a> {
    pub repo: &'a Repository,
    pub cancel: &'a Cancel,
}

pub async fn prepare<'a>(
    request: PrepareRequest<'a>,
    context: &PrepareContext<'a>,
    mut game_components: Components,
) -> Result<Components> {
    context.cancel.check()?;

    let profile = metadata::profile(
        context.repo,
        request.loader_kind,
        &request.game_version,
        &request.loader_version,
    )
    .await?;

    context.cancel.check()?;

    game_components.manifest = modloader::profile::merge_official(
        &game_components.manifest,
        &profile.manifest,
        &request.game_version,
    )?;

    game_components.pins.push(profile.pin);

    Ok(game_components)
}
