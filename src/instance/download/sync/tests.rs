use std::sync::Mutex;

use async_broadcast::Receiver;
use httpmock::{Method::GET, Mock, MockServer};
use sha1::{Digest, Sha1};
use tempfile::{tempdir, TempDir};

use crate::{
    bus::{event::CoreEvent, EventBus},
    data::registry::operation::{
        message::{
            event::{OperationEvent, OperationUpdate},
            status::Progress,
            OperationMessage,
        },
        OperationRegistry,
    },
};

use super::*;

const STAGE: OperationStage = OperationStage::DownloadAssets;
const BUFFER_SIZE: usize = 4096;
const CONCURRENCY: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestItem {
    name: String,
    hash: String,
    url: String,
}

impl Downloadable for TestItem {
    fn get_name(&self) -> &String {
        &self.name
    }

    fn get_hash(&self) -> &String {
        &self.hash
    }

    fn get_url(&self) -> &String {
        &self.url
    }
}

/// Хранилище в памяти.
///
/// `cached` намеренно фильтрует по `wanted`, а не отдаёт всё, что знает:
/// таков контракт `WHERE hash IN (...)` у настоящих сторов, и реализация,
/// которая проигнорировала бы аргумент, обязана валить тест, а не проходить.
#[derive(Default)]
struct FakeStore {
    known: HashSet<String>,
    registered: Mutex<Vec<String>>,
    register_calls: Mutex<usize>,
}

impl FakeStore {
    fn with_known(hashes: impl IntoIterator<Item = String>) -> Self {
        Self {
            known: hashes.into_iter().collect(),
            ..Default::default()
        }
    }

    fn registered(&self) -> Vec<String> {
        let mut hashes = self.registered.lock().unwrap().clone();
        hashes.sort();
        hashes
    }

    fn register_calls(&self) -> usize {
        *self.register_calls.lock().unwrap()
    }
}

#[async_trait]
impl SyncStore<TestItem> for FakeStore {
    async fn cached(&self, wanted: &[TestItem]) -> Result<HashSet<String>, DbError> {
        Ok(wanted
            .iter()
            .map(|item| item.hash.clone())
            .filter(|hash| self.known.contains(hash))
            .collect())
    }

    async fn register(&self, items: &[TestItem]) -> Result<(), DbError> {
        *self.register_calls.lock().unwrap() += 1;

        self.registered
            .lock()
            .unwrap()
            .extend(items.iter().map(|item| item.hash.clone()));

        Ok(())
    }
}

struct Harness {
    _tmp: TempDir,
    dir: PathBuf,
    bus: EventBus,
    ops: OperationRegistry,
}

impl Harness {
    fn new() -> Self {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        // Ёмкость с большим запасом: подписчик не опрашивается до конца
        // прогона, а шина при переполнении вытесняет самое старое — то есть
        // ровно то первое событие, ради которого половина тестов написана.
        let bus = EventBus::new(1024);
        let ops = OperationRegistry::new(bus.clone());

        Self {
            _tmp: tmp,
            dir,
            bus,
            ops,
        }
    }

    /// Подписка строго до `begin`: у шины нет истории, и всё, что
    /// опубликовано раньше первого `subscribe`, теряется без следа.
    fn operation(&self) -> (OperationHandle, Receiver<CoreEvent>) {
        let rx = self.bus.subscribe();
        let op = self.ops.begin("test".to_string(), vec![STAGE]);

        (op, rx)
    }
}

fn syncer() -> Syncer {
    Syncer::new(CONCURRENCY, BUFFER_SIZE)
}

fn sha1_hex(body: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(body);

    format!("{:x}", hasher.finalize())
}

fn item(name: &str, url: String, body: &[u8]) -> TestItem {
    TestItem {
        name: name.to_string(),
        hash: sha1_hex(body),
        url,
    }
}

async fn serve<'a>(server: &'a MockServer, path: &str, status: u16, body: &[u8]) -> Mock<'a> {
    let path = path.to_string();
    let body = body.to_vec();

    server
        .mock_async(move |when, then| {
            when.method(GET).path(path);
            then.status(status).body(body);
        })
        .await
}

/// Все точки прогресса стадии в порядке публикации.
fn progress(rx: &mut Receiver<CoreEvent>) -> Vec<(usize, usize)> {
    let mut seen = Vec::new();

    while let Ok(event) = rx.try_recv() {
        let CoreEvent::Operation(OperationMessage { data, .. }) = event else {
            continue;
        };

        let OperationEvent::Update(OperationUpdate::Progress { progress, .. }) = data else {
            continue;
        };

        if let Progress::Determinable { current, total } = progress {
            seen.push((current, total));
        }
    }

    seen
}

#[async_std::test]
async fn full_cache_reports_completion_immediately() {
    let hx = Harness::new();
    let (op, mut rx) = hx.operation();

    let bodies: [&[u8]; 3] = [b"one", b"two", b"three"];
    let wanted: Vec<TestItem> = bodies
        .iter()
        .enumerate()
        // Порт 1 никто не слушает: если реализация всё же полезет в сеть,
        // тест упадёт на отказе в соединении, а не зависнет на таймауте.
        .map(|(i, body)| item(&format!("f{i}"), "http://127.0.0.1:1/never".to_string(), body))
        .collect();

    let store = FakeStore::with_known(wanted.iter().map(|item| item.hash.clone()));

    let outcome = syncer()
        .run(wanted, |item| hx.dir.join(&item.name), &store, &op, STAGE)
        .await
        .expect("nothing to download");

    assert_eq!(outcome.downloaded.len(), 0);
    assert_eq!(outcome.cached.len(), 3);
    assert_eq!(
        store.register_calls(),
        0,
        "register must not be called with an empty batch"
    );

    // Ради этого прогресс и считается от `cached_count`: при полном кэше
    // первое же событие показывает 100%, а не «0 из 3».
    assert_eq!(progress(&mut rx).first().copied(), Some((3, 3)));
}

#[async_std::test]
async fn empty_cache_downloads_everything() {
    let server = MockServer::start_async().await;
    let hx = Harness::new();
    let (op, mut rx) = hx.operation();

    let bodies: [&[u8]; 3] = [b"alpha", b"beta", b"gamma"];
    let mut wanted = Vec::new();

    for (i, body) in bodies.iter().enumerate() {
        let path = format!("/f{i}");
        serve(&server, &path, 200, body).await;
        wanted.push(item(&format!("f{i}"), server.url(&path), body));
    }

    let store = FakeStore::default();

    let outcome = syncer()
        .run(
            wanted.clone(),
            |item| hx.dir.join(&item.name),
            &store,
            &op,
            STAGE,
        )
        .await
        .expect("all downloads succeed");

    assert_eq!(outcome.cached.len(), 0);
    assert_eq!(outcome.downloaded.len(), 3);
    assert_eq!(
        store.register_calls(),
        1,
        "successes go to the store in a single batch"
    );

    let mut expected: Vec<String> = wanted.iter().map(|item| item.hash.clone()).collect();
    expected.sort();
    assert_eq!(store.registered(), expected);

    for item in &wanted {
        assert!(
            hx.dir.join(&item.name).exists(),
            "{} must be on disk",
            item.name
        );
    }

    assert_eq!(progress(&mut rx).last().copied(), Some((3, 3)));
}

#[async_std::test]
async fn duplicates_are_downloaded_once() {
    let server = MockServer::start_async().await;
    let hx = Harness::new();
    let (op, mut rx) = hx.operation();

    let body: &[u8] = b"same content";
    let mock = serve(&server, "/dup", 200, body).await;

    // Один и тот же артефакт приходит из нескольких веток `requires`.
    // Без дедупа это две параллельные загрузки в один файл.
    let wanted = vec![
        item("first", server.url("/dup"), body),
        item("second", server.url("/dup"), body),
    ];

    let store = FakeStore::default();

    let outcome = syncer()
        .run(wanted, |item| hx.dir.join(&item.name), &store, &op, STAGE)
        .await
        .expect("duplicate collapses");

    assert_eq!(outcome.downloaded.len(), 1);
    assert_eq!(
        mock.hits_async().await,
        1,
        "the same hash must not be fetched twice"
    );
    assert_eq!(progress(&mut rx).last().copied(), Some((1, 1)));
}

#[async_std::test]
async fn transient_failure_is_retried_up_to_the_cap() {
    let server = MockServer::start_async().await;
    let hx = Harness::new();
    let (op, _rx) = hx.operation();

    let mock = serve(&server, "/flaky", 503, b"").await;
    let wanted = vec![item("flaky", server.url("/flaky"), b"payload")];

    let store = FakeStore::default();

    let err = syncer()
        .run(wanted, |item| hx.dir.join(&item.name), &store, &op, STAGE)
        .await
        .expect_err("503 on every attempt must end as a failure");

    // Проверяется число обращений, а не «в итоге получилось»: httpmock не
    // умеет отвечать по-разному на одинаковые запросы, а подделка через
    // счётчик в матчере завязала бы тест на порядок, в котором сервер
    // перебирает моки. Счётчик обращений говорит ровно то же самое.
    assert_eq!(
        mock.hits_async().await,
        MAX_ATTEMPTS,
        "503 is transient, it must be retried up to the cap"
    );
    assert!(matches!(err, SyncError::DownloadsFailed { failed: 1, .. }));
}

#[async_std::test]
async fn permanent_failure_is_not_retried() {
    let server = MockServer::start_async().await;
    let hx = Harness::new();
    let (op, _rx) = hx.operation();

    let mock = serve(&server, "/gone", 404, b"").await;
    let wanted = vec![item("gone", server.url("/gone"), b"whatever")];

    let store = FakeStore::default();

    let err = syncer()
        .run(wanted, |item| hx.dir.join(&item.name), &store, &op, STAGE)
        .await
        .expect_err("404 must fail the stage");

    assert_eq!(
        mock.hits_async().await,
        1,
        "404 is not transient, retrying it only wastes time"
    );
    assert!(matches!(
        err,
        SyncError::DownloadsFailed {
            failed: 1,
            total: 1,
            ..
        }
    ));
}

#[async_std::test]
async fn checksum_mismatch_is_not_retried() {
    let server = MockServer::start_async().await;
    let hx = Harness::new();
    let (op, _rx) = hx.operation();

    let mock = serve(&server, "/corrupt", 200, b"actual body").await;
    let wanted = vec![item("corrupt", server.url("/corrupt"), b"expected body")];

    let store = FakeStore::default();

    let err = syncer()
        .run(wanted, |item| hx.dir.join(&item.name), &store, &op, STAGE)
        .await
        .expect_err("checksum mismatch must fail the stage");

    // Фиксирует осознанное решение, а не самоочевидную истину: обрыв тела
    // на середине даёт ту же ошибку и повтором бы лечился, но её же даёт
    // неверный хэш в манифесте, где повторять нечего. Развести эти два
    // случая можно только сверкой длины с `Content-Length`.
    assert_eq!(
        mock.hits_async().await,
        1,
        "checksum mismatch is classified as permanent"
    );
    assert!(matches!(err, SyncError::DownloadsFailed { failed: 1, .. }));
}

#[async_std::test]
async fn partial_failure_still_registers_successes() {
    let server = MockServer::start_async().await;
    let hx = Harness::new();
    let (op, _rx) = hx.operation();

    let kept: &[u8] = b"kept";
    serve(&server, "/ok", 200, kept).await;
    serve(&server, "/bad", 404, b"").await;

    let wanted = vec![
        item("ok", server.url("/ok"), kept),
        item("bad", server.url("/bad"), b"never"),
    ];

    let store = FakeStore::default();

    let err = syncer()
        .run(wanted, |item| hx.dir.join(&item.name), &store, &op, STAGE)
        .await
        .expect_err("one failure fails the stage");

    assert!(matches!(
        err,
        SyncError::DownloadsFailed {
            failed: 1,
            total: 2,
            ..
        }
    ));

    // Иначе повторная установка качает заново и то, что уже лежит на диске:
    // `cached` спрашивает хранилище, а не файловую систему.
    assert_eq!(
        store.registered(),
        vec![sha1_hex(kept)],
        "successes must be registered before the error is returned"
    );
}

#[async_std::test]
async fn progress_never_goes_backwards() {
    let server = MockServer::start_async().await;
    let hx = Harness::new();
    let (op, mut rx) = hx.operation();

    let cached_bodies: [&[u8]; 2] = [b"cached one", b"cached two"];
    let fresh_bodies: [&[u8]; 3] = [b"fresh one", b"fresh two", b"fresh three"];

    let mut wanted = Vec::new();

    for (i, body) in cached_bodies.iter().enumerate() {
        wanted.push(item(
            &format!("c{i}"),
            "http://127.0.0.1:1/never".to_string(),
            body,
        ));
    }

    let known: Vec<String> = wanted.iter().map(|item| item.hash.clone()).collect();

    for (i, body) in fresh_bodies.iter().enumerate() {
        let path = format!("/n{i}");
        serve(&server, &path, 200, body).await;
        wanted.push(item(&format!("n{i}"), server.url(&path), body));
    }

    let store = FakeStore::with_known(known);

    syncer()
        .run(wanted, |item| hx.dir.join(&item.name), &store, &op, STAGE)
        .await
        .expect("fresh downloads succeed");

    let seen = progress(&mut rx);

    assert!(
        seen.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "current must never decrease: {seen:?}"
    );
    assert!(
        seen.iter().all(|(_, total)| *total == 5),
        "total is the whole set, not this session's downloads: {seen:?}"
    );
    assert_eq!(
        seen.first().copied(),
        Some((2, 5)),
        "the cached ones count from the very first event"
    );
    assert_eq!(seen.last().copied(), Some((5, 5)));
}
