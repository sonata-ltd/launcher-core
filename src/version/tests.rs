use async_channel::{Receiver, Sender};
use async_trait::async_trait;
use chrono::Utc;
use futures::join;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use super::{
    cache::{self, DiskIndex},
    catalog::{CatalogSource, VersionCatalog},
    error::{Result, VersionError},
    provider::MetaProvider,
    transport::{read_response, IndexLoader, MAX_INDEX_BYTES},
};

const PROVIDER: MetaProvider = MetaProvider::Prism;

fn index(id: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "formatVersion": 1,
        "name": "Minecraft",
        "uid": "net.minecraft",
        "versions": [{
            "recommended": true,
            "releaseTime": "2026-01-01T00:00:00Z",
            "sha256": "fixture-hash",
            "type": "release",
            "version": id
        }]
    }))
    .unwrap()
}

struct ControlledLoader {
    calls: Arc<AtomicUsize>,
    started: Sender<MetaProvider>,
    replies: Receiver<Result<Vec<u8>>>,
}

#[async_trait]
impl IndexLoader for ControlledLoader {
    async fn load(&self, provider: MetaProvider) -> Result<Vec<u8>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Most tests do not observe starts and drop that receiver.
        let _ = self.started.send(provider).await;
        self.replies
            .recv()
            .await
            .map_err(|error| VersionError::IndexUnavailable(error.to_string()))?
    }
}

fn harness(
    root: Option<std::path::PathBuf>,
) -> (
    Arc<VersionCatalog>,
    Sender<Result<Vec<u8>>>,
    Receiver<MetaProvider>,
    Arc<AtomicUsize>,
) {
    let (started, requests) = async_channel::unbounded();
    let (replies, responses) = async_channel::unbounded();
    let calls = Arc::new(AtomicUsize::new(0));
    let catalog = VersionCatalog::build(
        PROVIDER,
        root,
        Arc::new(ControlledLoader {
            calls: Arc::clone(&calls),
            started,
            replies: responses,
        }),
    );
    (Arc::new(catalog), replies, requests, calls)
}

#[async_std::test]
async fn fresh_disk_cache_survives_new_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let (catalog, replies, _, _) = harness(Some(dir.path().into()));
    replies.send(Ok(index("1.21.4"))).await.unwrap();
    catalog.refresh_from(PROVIDER).await.unwrap();
    drop(catalog);

    let (reopened, _, _, calls) = harness(Some(dir.path().into()));
    let cached = reopened.cached_snapshot(PROVIDER).await.unwrap().unwrap();
    assert_eq!(cached.source, CatalogSource::Disk);
    assert!(!cached.stale);
    assert_eq!(cached.versions[0].id, "1.21.4");
    assert_eq!(reopened.list_from(PROVIDER).await.unwrap()[0].id, "1.21.4");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[async_std::test]
async fn old_and_future_disk_dates_are_stale() {
    for date in [
        Utc::now() - chrono::Duration::hours(7),
        Utc::now() + chrono::Duration::hours(1),
    ] {
        let dir = tempfile::tempdir().unwrap();
        cache::write(
            dir.path().into(),
            PROVIDER,
            DiskIndex::new(PROVIDER, date, String::from_utf8(index("old")).unwrap()),
        )
        .await
        .unwrap();
        let (catalog, _, _, _) = harness(Some(dir.path().into()));
        let cached = catalog.cached_snapshot(PROVIDER).await.unwrap().unwrap();
        assert!(cached.stale);
        assert_eq!(cached.versions[0].id, "old");
    }
}

#[async_std::test]
async fn failed_refresh_preserves_memory_and_disk() {
    let dir = tempfile::tempdir().unwrap();
    let (catalog, replies, _, _) = harness(Some(dir.path().into()));
    replies.send(Ok(index("old"))).await.unwrap();
    catalog.refresh_from(PROVIDER).await.unwrap();

    replies.send(Ok(b"invalid JSON".to_vec())).await.unwrap();
    assert!(catalog.refresh_from(PROVIDER).await.is_err());
    assert_eq!(
        catalog
            .cached_snapshot(PROVIDER)
            .await
            .unwrap()
            .unwrap()
            .versions[0]
            .id,
        "old"
    );
    let disk = cache::read(dir.path().into(), PROVIDER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        PROVIDER
            .source()
            .unwrap()
            .parse_index(disk.raw.as_bytes())
            .unwrap()[0]
            .id,
        "old"
    );
}

#[async_std::test]
async fn concurrent_refreshes_share_one_request() {
    let (catalog, replies, requests, calls) = harness(None);
    let ((a, b), ()) = join!(
        async {
            join!(
                catalog.refresh_from(PROVIDER),
                catalog.refresh_from(PROVIDER)
            )
        },
        async {
            requests.recv().await.unwrap();
            replies.send(Ok(index("shared"))).await.unwrap();
        },
    );
    assert_eq!(a.unwrap().versions[0].id, "shared");
    assert_eq!(b.unwrap().versions[0].id, "shared");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[async_std::test]
async fn dropping_first_waiter_does_not_cancel_fetch() {
    let (catalog, replies, requests, calls) = harness(None);
    let first = Box::pin(catalog.refresh_from(PROVIDER));
    // Poll first until the independent task has signalled it started.
    match futures::future::select(first, Box::pin(requests.recv())).await {
        futures::future::Either::Right((request, waiting_future)) => {
            request.unwrap();
            drop(waiting_future);
        }
        _ => panic!("fetch must wait for a supplied response"),
    }
    let (second, ()) = join!(catalog.refresh_from(PROVIDER), async {
        replies.send(Ok(index("survived"))).await.unwrap();
    },);
    assert_eq!(second.unwrap().versions[0].id, "survived");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[async_std::test]
async fn write_failure_is_a_warning_with_usable_memory() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("a-file-not-a-directory");
    std::fs::write(&root, b"occupied").unwrap();
    let (catalog, replies, _, _) = harness(Some(root));
    replies.send(Ok(index("memory-only"))).await.unwrap();
    let snapshot = catalog.refresh_from(PROVIDER).await.unwrap();
    assert!(snapshot.warning.is_some());
    assert_eq!(
        catalog
            .cached_snapshot(PROVIDER)
            .await
            .unwrap()
            .unwrap()
            .versions[0]
            .id,
        "memory-only"
    );
}

#[async_std::test]
async fn unsupported_provider_returns_error() {
    let catalog = VersionCatalog::new(PROVIDER);
    assert!(matches!(
        catalog.cached_snapshot(MetaProvider::Mojang).await,
        Err(VersionError::ProviderUnsupported(_)),
    ));
    assert!(matches!(
        catalog.refresh_from(MetaProvider::Mojang).await,
        Err(VersionError::ProviderUnsupported(_)),
    ));
}

#[test]
fn parser_rejects_wrong_component_and_duplicate_ids() {
    let original: serde_json::Value = serde_json::from_slice(&index("duplicate")).unwrap();
    let mut wrong = original.clone();
    wrong["uid"] = "net.fabricmc.intermediary".into();
    assert!(PROVIDER
        .source()
        .unwrap()
        .parse_index(&serde_json::to_vec(&wrong).unwrap())
        .is_err());

    let mut duplicate = original;
    let entry = duplicate["versions"][0].clone();
    duplicate["versions"].as_array_mut().unwrap().push(entry);
    assert!(PROVIDER
        .source()
        .unwrap()
        .parse_index(&serde_json::to_vec(&duplicate).unwrap())
        .is_err());
}

#[async_std::test]
async fn corrupt_disk_can_be_replaced_by_network() {
    let dir = tempfile::tempdir().unwrap();
    let path = cache::file_path(dir.path(), PROVIDER);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    let (catalog, replies, _, _) = harness(Some(dir.path().into()));
    assert!(catalog.cached_snapshot(PROVIDER).await.is_err());
    replies.send(Ok(index("recovered"))).await.unwrap();
    assert_eq!(
        catalog.list_from(PROVIDER).await.unwrap()[0].id,
        "recovered"
    );
    assert!(cache::read(dir.path().into(), PROVIDER)
        .await
        .unwrap()
        .is_some());
}

#[async_std::test]
async fn http_error_is_not_parsed_as_an_index() {
    let mut response = http_types::Response::new(503);
    response.set_body("maintenance");
    assert!(matches!(
        read_response(response.into()).await,
        Err(VersionError::HttpStatus(503))
    ));
}

#[async_std::test]
async fn oversized_http_body_is_rejected() {
    let mut response = http_types::Response::new(200);
    response.set_body(vec![b'x'; MAX_INDEX_BYTES + 1]);
    assert!(matches!(
        read_response(response.into()).await,
        Err(VersionError::IndexTooLarge(_))
    ));
}

#[async_std::test]
async fn successful_http_body_is_returned_unchanged() {
    let mut response = http_types::Response::new(200);
    let body = index("valid");
    response.set_body(body.clone());
    assert_eq!(read_response(response.into()).await.unwrap(), body);
}

#[async_std::test]
async fn unknown_cache_schema_recovers_from_network() {
    let dir = tempfile::tempdir().unwrap();
    let path = cache::file_path(dir.path(), PROVIDER);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let value = serde_json::json!({
        "schema": 99, "provider": "prism", "component": "net.minecraft",
        "fetched_at": Utc::now(), "raw": String::from_utf8(index("old")).unwrap(),
    });
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    let (catalog, replies, _, _) = harness(Some(dir.path().into()));
    assert!(catalog.cached_snapshot(PROVIDER).await.is_err());
    replies.send(Ok(index("repaired"))).await.unwrap();
    assert_eq!(catalog.list_from(PROVIDER).await.unwrap()[0].id, "repaired");
}

#[async_std::test]
async fn concurrent_errors_are_shared_and_next_refresh_can_retry() {
    let (catalog, replies, requests, calls) = harness(None);
    let ((a, b), ()) = join!(
        async {
            join!(
                catalog.refresh_from(PROVIDER),
                catalog.refresh_from(PROVIDER)
            )
        },
        async {
            requests.recv().await.unwrap();
            replies
                .send(Err(VersionError::IndexUnavailable("offline".into())))
                .await
                .unwrap();
        },
    );
    assert!(a.is_err());
    assert!(b.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    replies.send(Ok(index("retry"))).await.unwrap();
    assert_eq!(
        catalog.refresh_from(PROVIDER).await.unwrap().versions[0].id,
        "retry"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
