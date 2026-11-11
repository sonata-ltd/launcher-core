use std::collections::HashMap;

use serde_json::{json, Value};

use crate::{
    modloader::{
        components::{self, Components},
        error::Result,
        files,
        model::MetadataPin,
        repository::Repository,
        transfer::Cancel,
    },
    version::provider::{prism::PrismSource, MetaProvider, VersionSource},
};

pub struct PrepareRequest<'a> {
    pub game_version: &'a str,
    pub manifest_url: &'a str,
    pub meta_provider: MetaProvider,
}

pub struct PrepareContext<'a> {
    pub repo: &'a Repository,
    pub cancel: &'a Cancel,
}

pub async fn prepare<'a>(
    request: PrepareRequest<'a>,
    context: PrepareContext<'a>,
) -> Result<Components> {
    context.cancel.check()?;

    let game_components = if request.meta_provider == MetaProvider::Prism {
        components::prism(
            context.repo,
            request.game_version,
            VersionSource::base_url(&PrismSource),
            HashMap::new(),
            vec![("net.minecraft".into(), request.game_version.to_string())],
        )
        .await?
    } else {
        let manifest_bytes = context.repo.bytes(request.manifest_url, None).await?;
        let manifest_value: Value = serde_json::from_slice(&manifest_bytes)?;

        let mut game_components = components::Components::default();
        components::apply(&mut game_components, &manifest_value)?;

        game_components.manifest["id"] = json!(request.game_version);
        game_components.pins.push(MetadataPin {
            url: request.manifest_url.to_string(),
            sha256: files::sha256(&manifest_bytes),
        });

        game_components
    };

    context.cancel.check()?;

    Ok(game_components)
}
