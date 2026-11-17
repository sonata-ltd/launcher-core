use futures::{stream, StreamExt};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::model::*;
use crate::utils::download::download_in_json;

#[derive(Deserialize)]
struct Asset {
    binary: Binary,
    version: Version,
}

#[derive(Deserialize)]
struct Version {
    openjdk_version: String,
    major: u32,
}

#[derive(Deserialize)]
struct Binary {
    architecture: String,
    os: String,
    image_type: String,
    package: Package,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    link: String,
    size: u64,
    checksum: String,
}

pub(super) async fn fetch(target: JavaTarget) -> Result<Vec<ManagedJavaPackage>> {
    let releases = download_in_json("https://api.adoptium.net/v3/info/available_releases")
        .await
        .map_err(|e| ManagedJavaError::new("catalog", e))?;
    let majors: Vec<u32> = releases["available_releases"]
        .as_array()
        .ok_or_else(|| ManagedJavaError::new("catalog", "Missing available_releases"))?
        .iter()
        .filter_map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
        .filter(|n| *n >= 8)
        .collect();
    let responses = stream::iter(majors).map(|major| {
        let target = target.clone();
        async move {
            let url = format!("https://api.adoptium.net/v3/assets/latest/{major}/hotspot?architecture={}&image_type=jre&os={}&vendor=eclipse", target.arch, target.os);
            match fetch_assets(&url).await {
                Ok(value) => parse(value, &target, major),
                Err(e) => Err(ManagedJavaError::new("catalog", format!("{url}: {e}"))),
            }
        }
    }).buffer_unordered(4).collect::<Vec<_>>().await;
    let mut packages = Vec::new();
    let mut errors = Vec::new();
    for response in responses {
        match response {
            Ok(mut found) => packages.append(&mut found),
            Err(error) => errors.push(error.diagnostic()),
        }
    }
    if !errors.is_empty() {
        return Err(ManagedJavaError::new("catalog", errors.join("\n\n")));
    }
    packages.sort_by(|a, b| b.major.cmp(&a.major));
    Ok(packages)
}

pub(super) fn parse(
    value: serde_json::Value,
    target: &JavaTarget,
    major: u32,
) -> Result<Vec<ManagedJavaPackage>> {
    let assets: Vec<Asset> =
        serde_json::from_value(value).map_err(|e| ManagedJavaError::new("catalog", e))?;
    let mut packages = Vec::new();
    for asset in assets {
        let b = asset.binary;
        if b.os != target.os
            || b.architecture != target.arch
            || b.image_type != "jre"
            || asset.version.major != major
        {
            continue;
        }
        let p = b.package;
        let format = if p.name.ends_with(".zip") {
            ArchiveFormat::Zip
        } else if p.name.ends_with(".tar.gz") {
            ArchiveFormat::TarGz
        } else {
            continue;
        };
        if p.size == 0
            || p.checksum.len() != 64
            || !p.checksum.bytes().all(|b| b.is_ascii_hexdigit())
            || !p.link.starts_with("https://")
        {
            return Err(ManagedJavaError::new(
                "catalog",
                "Invalid package size, SHA-256 or download URL",
            ));
        }
        let sha256 = p.checksum.to_lowercase();
        let id = format!(
            "temurin-{:x}",
            Sha256::digest(format!(
                "{}:{}:{}:{}",
                target.os, target.arch, b.image_type, sha256
            ))
        );
        packages.push(ManagedJavaPackage {
            id,
            version: asset.version.openjdk_version,
            major,
            target: target.clone(),
            image_type: b.image_type,
            url: p.link,
            size: p.size,
            sha256,
            format,
        });
    }
    Ok(packages)
}

async fn fetch_assets(url: &str) -> Result<serde_json::Value> {
    use async_std::future::timeout;
    use std::time::Duration;
    timeout(Duration::from_secs(60), async {
        let mut response = surf::get(url)
            .await
            .map_err(|e| ManagedJavaError::new("catalog", e))?;
        if response.status() == surf::StatusCode::NotFound {
            return Ok(serde_json::json!([]));
        }
        if !response.status().is_success() {
            return Err(ManagedJavaError::new(
                "catalog",
                format!("HTTP {}: {url}", response.status()),
            ));
        }
        response
            .body_json()
            .await
            .map_err(|e| ManagedJavaError::new("catalog", e))
    })
    .await
    .map_err(|_| ManagedJavaError::new("catalog", format!("Catalog request timed out: {url}")))?
}
