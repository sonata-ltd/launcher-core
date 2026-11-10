use std::path::PathBuf;

use super::{
    artifacts,
    catalog::ModloaderCatalog,
    components,
    error::{ModloaderError, Result},
    files,
    model::{InstalledProfile, LoaderKind, LoaderSpec, MetadataPin},
    transfer::Cancel,
};
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
        download::{assets::AssetsData, manifest::get_assets_manifest},
        model::InstanceRecord,
    },
    java::model::JavaRuntime,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc, time::Instant};

pub enum ArtifactSource {
    Download {
        url: String,
        expected_sha1: Option<String>,
    },
    Embedded {
        archive: PathBuf,
        entry: String,
        expected_sha1: Option<String>,
    },
    Generated {
        processor_index: usize,
        expected_sha1: Option<String>,
    },
}

pub struct ArtifactRequest {
    pub relative_path: PathBuf,
    pub source: ArtifactSource,
    pub classpath: bool,
    pub native: bool,
}

fn complete(op: &OperationHandle, stage: OperationStage, started: Instant) {
    op.complete_stage(StageResult {
        stage,
        status: Outcome::Completed,
        duration_secs: started.elapsed().as_secs_f64(),
        error: None,
    });
}

pub async fn install(
    rec: &InstanceRecord,
    paths: &LauncherPaths,
    db: Arc<Database>,
    catalog: &ModloaderCatalog,
    java: &[Arc<JavaRuntime>],
    op: &OperationHandle,
    cancel: &Cancel,
) -> Result<InstalledProfile> {
    cancel.check()?;

    let kind = LoaderKind::ALL
        .into_iter()
        .find(|kind| kind.as_str() == rec.loader)
        .ok_or_else(|| ModloaderError::Invalid("unknown loader".into()))?;

    let spec = LoaderSpec {
        kind,
        version: rec.loader_version.clone(),
    };

    spec.validate(&rec.version)?;

    let relative_root = PathBuf::from("instances")
        .join(rec.id.to_string())
        .join(uuid::Uuid::new_v4().to_string())
        .join("maven");

    let maven = paths.libraries().join(&relative_root);
    async_std::fs::create_dir_all(&maven).await?;

    let temp = maven
        .parent()
        .ok_or_else(|| ModloaderError::Invalid("Maven path".into()))?
        .join("work");

    async_std::fs::create_dir_all(&temp).await?;

    let previous = super::storage::load(&db, rec.id)
        .await
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    let pinned = catalog.repository.pinned(
        previous
            .as_ref()
            .filter(|p| p.loader == spec && p.minecraft == rec.version)
            .map(|p| p.metadata.as_slice())
            .unwrap_or(&[]),
    );

    let repo = &pinned;
    let blobs = paths.libraries().join("blobs");

    let started = Instant::now();
    op.start_stage(OperationStage::FetchManifest);

    let mut base = if kind == LoaderKind::Cleanroom {
        let releases = super::cleanroom::releases(repo).await?;
        let release = releases
            .iter()
            .find(|v| Some(&v.version) == spec.version.as_ref())
            .ok_or_else(|| ModloaderError::Invalid("Cleanroom release is not registered".into()))?;

        super::cleanroom::prepare(repo, release, temp.as_path(), &maven, cancel).await?
    } else if rec.meta_provider.eq_ignore_ascii_case("prism") {
        components::prism(
            repo,
            &rec.version,
            "https://meta.prismlauncher.org/v1/",
            HashMap::new(),
            vec![("net.minecraft".into(), rec.version.clone())],
        )
        .await?
    } else {
        let bytes = repo.bytes(&rec.manifest_url, None).await?;
        let value: Value = serde_json::from_slice(&bytes)?;

        let mut base = components::Components::default();
        components::apply(&mut base, &value)?;

        base.manifest["id"] = json!(rec.version);
        base.pins.push(MetadataPin {
            url: rec.manifest_url.clone(),
            sha256: files::sha256(&bytes),
        });

        base
    };

    let mut forge = None;

    if matches!(kind, LoaderKind::Fabric | LoaderKind::Quilt) {
        let version = spec
            .version
            .as_deref()
            .ok_or_else(|| ModloaderError::Invalid("missing loader version".into()))?;

        let patch = super::fabric_quilt::profile(repo, kind, &rec.version, version).await?;

        let endpoint = if kind == LoaderKind::Fabric {
            "https://meta.fabricmc.net/v2/versions/loader/"
        } else {
            "https://meta.quiltmc.org/v3/versions/loader/"
        };

        let url = super::repository::url(endpoint, &[&rec.version, version, "profile", "json"])?;
        let bytes = repo.bytes(&url, None).await?;

        base.manifest = super::profile::merge_official(&base.manifest, &patch, &rec.version)?;
        base.pins.push(MetadataPin {
            url,
            sha256: files::sha256(&bytes),
        });
    } else if matches!(kind, LoaderKind::Forge | LoaderKind::NeoForge) {
        let entry = catalog
            .versions(kind, &rec.version)
            .await?
            .into_iter()
            .find(|v| Some(&v.version) == spec.version.as_ref())
            .ok_or_else(|| ModloaderError::Invalid("loader not compatible with MC".into()))?;

        let url = entry
            .component_url
            .ok_or_else(|| ModloaderError::Invalid("missing component URL".into()))?;

        let bytes = repo.bytes(&url, entry.sha256.as_deref()).await?;
        let value: Value = serde_json::from_slice(&bytes)?;

        base.pins.push(MetadataPin {
            url,
            sha256: files::sha256(&bytes),
        });

        if value.get("mainClass").and_then(Value::as_str)
            == Some("io.github.zekerzhayard.forgewrapper.installer.Main")
        {
            let jar = super::forge::installer(
                repo,
                &value,
                &temp.as_path().join("installer.jar"),
                cancel,
            )
            .await?;

            forge = Some(
                super::forge::prepare(
                    jar,
                    temp.as_path().join("installer"),
                    maven.clone(),
                    &rec.version,
                    &mut base,
                )
                .await?,
            );
        } else {
            if kind == LoaderKind::NeoForge {
                return Err(ModloaderError::Unsupported(
                    "NeoForge component entrypoint schema".into(),
                ));
            }

            components::apply(&mut base, &value)?;
        }
    }

    let policy = components::policy(&base.manifest)?;

    complete(op, OperationStage::FetchManifest, started);

    let wanted = artifacts::parse(&mut base.manifest, &base.maven_files)?;

    let started = Instant::now();
    op.start_stage(OperationStage::DownloadLibs);

    artifacts::hydrate(&wanted, &maven, &blobs, cancel, op).await?;

    complete(op, OperationStage::DownloadLibs, started);

    let started = Instant::now();
    op.start_stage(OperationStage::CreateStructure);

    if let Some(prepared) = forge {
        let java = java
            .iter()
            .filter(|j| {
                j.arch.as_deref().is_none_or(|a| {
                    crate::java::select::normalize_arch(a) == crate::java::select::host_arch()
                })
            })
            .max_by_key(|j| j.major)
            .ok_or_else(|| {
                ModloaderError::Invalid("register a Java runtime for installer processors".into())
            })?;

        let client = base
            .manifest
            .get("mainJar")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .map(super::maven::MavenCoordinate::parse)
            .transpose()?
            .map(|v| v.path())
            .unwrap_or(
                super::maven::MavenCoordinate::parse(&format!(
                    "com.mojang:minecraft:{}:client",
                    rec.version
                ))?
                .path(),
            );

        super::forge::run(
            prepared,
            maven.clone(),
            maven.join(client),
            paths.instances().join(&rec.dir),
            PathBuf::from(&java.exec_path),
            cancel,
            op,
        )
        .await?;
    }

    complete(op, OperationStage::CreateStructure, started);
    cancel.check()?;

    let (assets, _) = get_assets_manifest(&base.manifest, paths.assets().join("indexes"))
        .await
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    AssetsData::sync(&assets, paths, Arc::clone(&db), op)
        .await
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    let started = Instant::now();
    op.start_stage(OperationStage::VerifyFiles);

    let artifacts = artifacts::finish(wanted, maven, op).await?;

    cancel.check()?;

    let profile = InstalledProfile {
        format_version: 1,
        minecraft: rec.version.clone(),
        loader: spec,
        manifest: base.manifest,
        java: policy,
        libraries_root: relative_root,
        artifacts,
        metadata: base.pins,
    };

    super::storage::publish(&db, rec.id, &profile)
        .await
        .map_err(|e| ModloaderError::Invalid(e.to_string()))?;

    complete(op, OperationStage::VerifyFiles, started);

    Ok(profile)
}
