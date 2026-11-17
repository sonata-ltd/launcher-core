use crate::instance::init::model::InstalledArtifact;
use crate::{
    data::registry::operation::message::stage::OperationStage,
    instance::{
        download::{
            http,
            transfer::{self, Cancel},
        },
        rules::{self, RuleContext},
    },
    utils::{files, maven::MavenCoordinate, url},
};

pub mod error;
use error::{ArtifactError, Result};

#[cfg(test)]
mod tests;
use futures::{stream, StreamExt, TryFutureExt, TryStreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub fn host() -> RuleContext<'static> {
    RuleContext {
        os: if cfg!(windows) {
            "windows"
        } else if cfg!(target_os = "macos") {
            "osx"
        } else {
            "linux"
        },
        arch: std::env::consts::ARCH,
        os_version: "",
        features: HashMap::new(),
    }
}

#[derive(Clone)]
pub struct Wanted {
    pub path: PathBuf,
    pub url: Option<String>,
    pub sha1: Option<String>,
    pub classpath: bool,
    pub native: bool,
}

fn descriptor(
    value: &Value,
    coordinate: &str,
    repository: Option<&str>,
    classpath: bool,
    native: bool,
) -> Result<Wanted> {
    let maven = MavenCoordinate::parse(coordinate)?;
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| maven.path());

    files::relative(&path)?;

    let url = value
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| repository.and_then(|base| url::relative_url(base, &path).ok()));
    let sha1 = value
        .get("sha1")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    if sha1
        .as_ref()
        .is_some_and(|hash| !files::hash_valid(hash, 40))
    {
        return Err(ArtifactError::Invalid("invalid artifact hash".into()));
    }

    Ok(Wanted {
        path,
        url,
        sha1,
        classpath,
        native,
    })
}

pub fn parse(manifest: &mut Value, maven_files: &[Value]) -> Result<Vec<Wanted>> {
    let mut result = Vec::new();
    let context = host();
    let mut excludes = serde_json::Map::new();

    for (field, cp) in [("libraries", true), ("mavenFiles", false)] {
        let values = if field == "libraries" {
            manifest
                .get(field)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        } else {
            maven_files.to_vec()
        };

        for library in values {
            if let Some(rules) = library.get("rules") {
                if !rules::rules_allow(
                    rules.as_array().ok_or_else(|| {
                        ArtifactError::Invalid("library rules must be array".into())
                    })?,
                    &context,
                )? {
                    continue;
                }
            }

            let name = library
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| ArtifactError::Invalid("library has no Maven coordinate".into()))?;

            let repo = library.get("url").and_then(Value::as_str);
            let downloads = library.get("downloads");

            if let Some(artifact) = downloads.and_then(|d| d.get("artifact")) {
                result.push(descriptor(artifact, name, None, cp, false)?);
            } else if downloads.is_none() {
                result.push(descriptor(
                    &json!({}),
                    name,
                    Some(repo.unwrap_or("https://libraries.minecraft.net/")),
                    cp,
                    false,
                )?);
            }

            if let Some(natives) = library.get("natives").and_then(Value::as_object) {
                if let Some(native) = natives.get(context.os).and_then(Value::as_str) {
                    let classifier = native.replace(
                        "${arch}",
                        if cfg!(target_pointer_width = "64") {
                            "64"
                        } else {
                            "32"
                        },
                    );

                    let value = downloads
                        .and_then(|d| d.get("classifiers"))
                        .and_then(|c| c.get(&classifier))
                        .ok_or_else(|| {
                            ArtifactError::Unsupported(format!(
                                "missing native classifier: {classifier}"
                            ))
                        })?;

                    let m = MavenCoordinate::parse(name)?;
                    let coordinate = format!(
                        "{}:{}:{}:{}@{}",
                        m.group, m.artifact, m.version, classifier, m.extension
                    );

                    let wanted = descriptor(value, &coordinate, None, false, true)?;

                    excludes.insert(
                        wanted.path.to_string_lossy().into_owned(),
                        library
                            .pointer("/extract/exclude")
                            .cloned()
                            .unwrap_or(json!(["META-INF/"])),
                    );

                    result.push(wanted);
                }
            }
        }
    }

    if let Some(main) = manifest.get("mainJar") {
        let name = main
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ArtifactError::Invalid("mainJar has no name".into()))?;

        let value = main.pointer("/downloads/artifact").unwrap_or(&Value::Null);
        result.push(descriptor(value, name, None, true, false)?);
    } else if let Some(client) = manifest.pointer("/downloads/client") {
        let version = manifest
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ArtifactError::Invalid("missing MC id".into()))?;

        result.push(descriptor(
            client,
            &format!("com.mojang:minecraft:{version}:client"),
            None,
            true,
            false,
        )?);
    }

    if let Some(logging) = manifest.pointer("/logging/client/file") {
        let id = logging
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| ArtifactError::Invalid("logging file id missing".into()))?;

        let mut entry = descriptor(logging, "org.sonata:logging:1", None, false, false)?;
        entry.path = PathBuf::from("sonata-logging").join(id);

        files::relative(&entry.path)?;
        result.push(entry);
    }

    manifest["_sonata_native_excludes"] = Value::Object(excludes);

    let mut indices = HashMap::new();
    let mut unique: Vec<Wanted> = Vec::new();

    for wanted in result {
        if let Some(index) = indices.get(&wanted.path).copied() {
            let old: &mut Wanted = &mut unique[index];
            if old.sha1.is_some() && wanted.sha1.is_some() && old.sha1 != wanted.sha1 {
                return Err(ArtifactError::Invalid(
                    "different hashes for same artifact path".into(),
                ));
            }

            old.classpath |= wanted.classpath;
            old.native |= wanted.native;

            if old.sha1.is_none() {
                old.sha1 = wanted.sha1;
            }

            if old.url.is_none() {
                old.url = wanted.url;
            }
        } else {
            indices.insert(wanted.path.clone(), unique.len());
            unique.push(wanted);
        }
    }

    Ok(unique)
}

pub async fn hydrate(
    wanted: &[Wanted],
    root: &Path,
    blobs: &Path,
    cancel: &Cancel,
    op: &crate::data::registry::operation::handle::OperationHandle,
) -> Result<()> {
    use crate::data::registry::operation::message::{
        stage::OperationStage,
        target::{FileStatus, ProcessTarget},
    };

    let wanted: Vec<_> = wanted
        .iter()
        .filter(|item| item.url.is_some() || root.join(&item.path).is_file())
        .cloned()
        .collect();

    let total = wanted.len();
    let mut completed = 0;

    op.update_stage(OperationStage::DownloadLibs, 0, total, None);

    stream::iter(wanted.iter().cloned().map(|wanted| {
        let root = root.to_owned();
        let blobs = blobs.to_owned();
        let cancel = cancel.clone();

        let name = wanted.path.to_string_lossy().into_owned();
        let work = async move {
            cancel.check()?;
            let path = files::child(&root, &wanted.path)?;

            if path.is_file() {
                if let Some(hash) = &wanted.sha1 {
                    let path = path.clone();
                    let hash = hash.clone();

                    if async_std::task::spawn_blocking(move || files::matches(&path, &hash)).await?
                    {
                        return Ok(());
                    }
                } else {
                    return Ok(());
                }
            }
            if let Some(url) = wanted.url {
                let sidecar = if wanted.sha1.is_none() {
                    let mut checksum_url = http_types::Url::parse(&url)
                        .map_err(|e| ArtifactError::Invalid(e.to_string()))?;
                    checksum_url.set_path(&format!("{}.sha1", checksum_url.path()));

                    http::get_bytes(checksum_url.as_str(), 4096)
                        .await
                        .ok()
                        .and_then(|bytes| String::from_utf8(bytes).ok())
                        .and_then(|body| body.split_whitespace().next().map(str::to_owned))
                        .filter(|hash| files::hash_valid(hash, 40))
                } else {
                    None
                };

                transfer::place(
                    &url,
                    &path,
                    wanted.sha1.as_deref().or(sidecar.as_deref()),
                    &blobs,
                    &cancel,
                )
                .await?;
            }

            Ok::<(), ArtifactError>(())
        };
        work.map_ok(move |_| name)
    }))
    .buffer_unordered(16)
    .inspect_ok(|name| {
        completed += 1;

        op.update_stage(
            OperationStage::DownloadLibs,
            completed,
            total,
            Some(ProcessTarget::file(name.clone(), FileStatus::Downloaded)),
        );
    })
    .try_collect::<Vec<_>>()
    .await?;

    Ok(())
}

pub async fn finish(
    wanted: Vec<Wanted>,
    root: PathBuf,
    op: &crate::data::registry::operation::handle::OperationHandle,
) -> Result<Vec<InstalledArtifact>> {
    op.update_stage(OperationStage::VerifyFiles, 0, wanted.len(), None);

    let verified = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = verified.clone();
    let total = wanted.len();

    let work = async_std::task::spawn_blocking(move || {
        let mut result = Vec::new();

        for wanted in wanted {
            let path = files::child(&root, &wanted.path)?;
            let hash = files::sha1_file(&path)?;

            if let Some(expected) = wanted
                .sha1
                .filter(|expected| !expected.eq_ignore_ascii_case(&hash))
            {
                return Err(ArtifactError::ChecksumMismatch {
                    path,
                    expected,
                    actual: hash,
                });
            }

            result.push(InstalledArtifact {
                path: wanted.path,
                sha1: hash,
                classpath: wanted.classpath,
                native: wanted.native,
            });

            count.fetch_add(1, std::sync::atomic::Ordering::Release);
        }

        Ok::<_, ArtifactError>(result)
    });

    futures::pin_mut!(work);

    loop {
        match futures::future::select(
            work.as_mut(),
            Box::pin(async_std::task::sleep(std::time::Duration::from_millis(50))),
        )
        .await
        {
            futures::future::Either::Left((result, _)) => {
                if result.is_ok() {
                    op.update_stage(OperationStage::VerifyFiles, total, total, None);
                }

                return result;
            }

            futures::future::Either::Right(_) => op.update_stage(
                OperationStage::VerifyFiles,
                verified.load(std::sync::atomic::Ordering::Acquire),
                total,
                None,
            ),
        }
    }
}
