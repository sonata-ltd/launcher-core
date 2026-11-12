use std::io::Write;

use serde_json::json;

use super::*;
use crate::{
    bus::{event::CoreEvent, EventBus},
    data::registry::operation::{
        message::{
            event::{OperationEvent, OperationUpdate},
            stage::StageStatus,
            status::{LifeCycle, Progress},
        },
        OperationRegistry,
    },
};

fn cache_metadata(cache: &Path, url: &str, value: &Value) -> MetadataPin {
    let bytes = serde_json::to_vec(value).unwrap();
    files::atomic_write(
        &cache.join(format!("{}.json", files::sha256(url.as_bytes()))),
        &bytes,
    )
    .unwrap();
    MetadataPin {
        url: url.into(),
        sha256: files::sha256(&bytes),
    }
}

fn write_installer(path: &Path, processors: Vec<Value>) -> String {
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, value) in [
        (
            "install_profile.json",
            json!({
                "spec": 1,
                "minecraft": "1.20.1",
                "json": "/version.json",
                "libraries": [{"name": "g:processor:1"}],
                "processors": processors,
            }),
        ),
        (
            "version.json",
            json!({
                "inheritsFrom": "1.20.1",
                "mainClass": "example.Bootstrap",
                "arguments": {"game": ["--launchTarget", "fixtureclient"]},
            }),
        ),
    ] {
        archive
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(&serde_json::to_vec(&value).unwrap())
            .unwrap();
    }
    archive
        .start_file(
            "maven/g/embedded/1/embedded-1.jar",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(b"embedded runtime").unwrap();
    archive.finish().unwrap();
    files::sha1_file(path).unwrap()
}

// None models a legacy component, Some models an actual installer archive.
// Both repositories are populated locally; the installer is already downloaded
// and must pass its SHA-1 check. Preparation needs neither network nor Java.
async fn assert_preparation(
    kind: LoaderKind,
    processors: Option<Vec<Value>>,
    expect_pending_processors: bool,
) {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("metadata");
    let work = temp.path().join("work");
    let maven = temp.path().join("maven");
    std::fs::create_dir_all(&work).unwrap();
    let modern = processors.is_some();
    let component = if let Some(processors) = processors {
        let sha1 = write_installer(&work.join("installer.jar"), processors);
        json!({
            "mainClass": "io.github.zekerzhayard.forgewrapper.installer.Main",
            "mavenFiles": [{
                "name": "g:loader:fixture:installer",
                "downloads": {"artifact": {
                    "url": "https://example.invalid/installer.jar",
                    "sha1": sha1,
                }},
            }],
        })
    } else {
        json!({
            "mainClass": "example.Bootstrap",
            "+tweakers": ["example.Tweaker"],
        })
    };
    let uid = match kind {
        LoaderKind::Forge => "net.minecraftforge",
        LoaderKind::NeoForge => "net.neoforged",
        _ => unreachable!(),
    };
    let component_url = format!("https://meta.prismlauncher.org/v1/{uid}/fixture.json");
    let component_pin = cache_metadata(&cache, &component_url, &component);
    let mut pins = vec![component_pin.clone()];
    for index_uid in ["net.minecraftforge", "net.neoforged"] {
        let versions = if index_uid == uid {
            json!([{
                "version": "fixture",
                "sha256": component_pin.sha256,
                "requires": [{"uid": "net.minecraft", "equals": "1.20.1"}],
            }])
        } else {
            json!([])
        };
        pins.push(cache_metadata(
            &cache,
            &format!("https://meta.prismlauncher.org/v1/{index_uid}/index.json"),
            &json!({"formatVersion": 1, "uid": index_uid, "versions": versions}),
        ));
    }
    let catalog = ModloaderCatalog {
        repository: Repository::new(cache).pinned(&pins),
    };
    let bus = EventBus::new(64);
    let mut events = bus.subscribe();
    let operations = OperationRegistry::new(bus);
    let op = operations.begin(
        "instance.install".into(),
        vec![
            OperationStage::FetchManifest,
            OperationStage::DownloadLibs,
            OperationStage::RunModloaderProcessors,
        ],
    );
    op.start_stage_determinable(OperationStage::FetchManifest);
    op.update_stage(OperationStage::FetchManifest, 1, 2, None);
    let cancel = Cancel::default();

    let prepared = prepare(
        PrepareRequest {
            game_version: "1.20.1".into(),
            loader_kind: kind,
            loader_version: "fixture".into(),
        },
        &PrepareContext {
            repo: &catalog.repository,
            catalog: &catalog,
            maven_dir: &maven,
            work_dir: &work,
            op: &op,
            cancel: &cancel,
        },
        Components {
            manifest: json!({
                "libraries": [],
                "arguments": {"game": ["--username", "Player"], "jvm": []},
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        prepared.pending_processors.is_some(),
        expect_pending_processors
    );
    if let Some(pending) = &prepared.pending_processors {
        assert!(pending.has_client_processors());
    }
    // Dropping Prepared when there is no work must not lose profile changes,
    // metadata pins or files extracted from the installer.
    assert_eq!(
        prepared.components.manifest["mainClass"],
        "example.Bootstrap"
    );
    let game = prepared.components.manifest["arguments"]["game"]
        .as_array()
        .unwrap();
    assert_eq!(&game[..2], &[json!("--username"), json!("Player")]);
    assert_eq!(prepared.components.pins.len(), 1);
    assert_eq!(prepared.components.pins[0].url, component_pin.url);
    assert_eq!(prepared.components.pins[0].sha256, component_pin.sha256);
    if modern {
        assert_eq!(
            prepared.components.maven_files,
            vec![json!({"name": "g:processor:1"})]
        );
        assert_eq!(
            std::fs::read(maven.join("g/embedded/1/embedded-1.jar")).unwrap(),
            b"embedded runtime"
        );
    }

    let mut skipped = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::Operation(message) = event {
            match message.data {
                OperationEvent::Update(OperationUpdate::Skipped { stage, reason }) => {
                    skipped.push((stage, reason));
                }
                OperationEvent::Update(OperationUpdate::Progress { stage, .. }) => {
                    assert_ne!(stage, OperationStage::RunModloaderProcessors);
                }
                OperationEvent::Update(OperationUpdate::Completed(result)) => {
                    assert_ne!(result.stage, OperationStage::RunModloaderProcessors);
                }
                _ => {}
            }
        }
    }
    if expect_pending_processors {
        assert!(skipped.is_empty());
    } else {
        assert_eq!(
            skipped,
            vec![(
                OperationStage::RunModloaderProcessors,
                Some("no client processors".into()),
            )]
        );
    }
    let snapshots = operations.snapshot_all();
    assert_eq!(snapshots.len(), 1);
    let snapshot = &snapshots[0];
    assert_eq!(snapshot.stage, Some(OperationStage::FetchManifest));
    assert!(matches!(snapshot.status, LifeCycle::InProgress));
    assert!(matches!(
        snapshot.progress,
        Progress::Determinable {
            current: 1,
            total: 2
        }
    ));
    let processors = snapshot
        .stages
        .iter()
        .find(|entry| entry.stage == OperationStage::RunModloaderProcessors)
        .unwrap();
    if expect_pending_processors {
        assert!(matches!(processors.status, StageStatus::Pending));
        assert!(processors.reason.is_none());
    } else {
        assert!(matches!(processors.status, StageStatus::Skipped));
        assert_eq!(processors.reason.as_deref(), Some("no client processors"));
    }
    op.finish(Outcome::Completed);
}

#[async_std::test]
async fn empty_installer_processors_are_skipped_once() {
    for kind in [LoaderKind::Forge, LoaderKind::NeoForge] {
        assert_preparation(kind, Some(vec![]), false).await;
    }
}

#[async_std::test]
async fn server_only_processors_are_skipped_once() {
    for kind in [LoaderKind::Forge, LoaderKind::NeoForge] {
        assert_preparation(
            kind,
            Some(vec![json!({
                "jar": "g:processor:1", "sides": ["server"],
            })]),
            false,
        )
        .await;
    }
}

#[async_std::test]
async fn client_processor_keeps_finalization_pending_without_skip() {
    for kind in [LoaderKind::Forge, LoaderKind::NeoForge] {
        assert_preparation(
            kind,
            Some(vec![
                json!({"jar": "g:processor:1", "sides": ["server"]}),
                json!({"jar": "g:processor:1", "sides": ["client"]}),
            ]),
            true,
        )
        .await;
    }
}

#[async_std::test]
async fn processor_without_sides_applies_to_client() {
    for kind in [LoaderKind::Forge, LoaderKind::NeoForge] {
        assert_preparation(kind, Some(vec![json!({"jar": "g:processor:1"})]), true).await;
    }
}

#[async_std::test]
async fn legacy_forge_without_installer_is_skipped_once() {
    assert_preparation(LoaderKind::Forge, None, false).await;
}
