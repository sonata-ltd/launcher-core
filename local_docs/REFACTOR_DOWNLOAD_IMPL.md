# Рефакторинг `instance/download/{libs,assets}` — реализация

## Context

`instance/download/*` — последняя часть среза `instance`, оставшаяся в старом поколении
архитектуры: `Result<_, String>`, прогресс закомментирован, импорты из удалённого модуля
`websocket`. Вокруг всё готово — `OperationRegistry`/`OperationHandle` с `EventBus` работают,
`LauncherPaths` резолвит раскладку, `VersionCatalog` отдаёт `VersionSummary`.

Две цели. Первая — компилирующийся и работающий `install`-путь. Вторая, более важная — устранить
главное дублирование проекта: два почти идентичных конвейера libs/assets (константы,
`process_futures`, полный SELECT + фильтр в памяти, `INSERT` в цикле). Третья копия появится, как
только дойдёт очередь до скачивания Java-рантайма.

Побочно закрывается баг сборки classpath, из-за которого запуск игры сейчас неработоспособен.

### Принятые решения

| Вопрос | Решение |
|---|---|
| Парсинг и загрузка | **Разделить.** `parse_manifest_*` → `манифест → Vec<LibInfo>`; рекурсия по `requires` собирает всё в один `Vec`; `Syncer` вызывается один раз |
| Границы | До `launch/` включительно: `sync.rs` + `libs/*` + `assets/*` + `init.rs` + `service.rs` + `LaunchInfoBuilder` |
| Провал загрузки | **Фатально для обоих.** Непустой `failed` → `Err` и у libs, и у assets |
| `SyncStore::cached` | `QueryBuilder` + `WHERE hash IN (...)`, чанки по 500 |

Отмечу как поднятый ранее риск: «фатально для assets» означает, что один недоступный объект из
нескольких тысяч отменяет всю установку. Решение принято сознательно; политика живёт в одном месте
(`Syncer::run` возвращает `Err`), смягчить её потом можно, добавив `failed` в `SyncOutcome` и
перенеся решение в вызывающего.

### Расхождения дока с кодом

`REFACTOR_INSTANCE_DOWNLOAD.md` §1.1 частично устарел: `init.rs::install` уже переписан на
`(version: VersionSummary, rec, paths: &LauncherPaths, op)`, переменных `client_data`/`register`/
`instance` там нет, `InstancePaths` в `install` не используется. Реально осталось: `state` не в
скоупе (`init.rs:83`) и `sync_libs` вызывается с 3 аргументами вместо 4 (`init.rs:48`).

Провайдер берётся из `version.provider` (`VersionSummary.provider: MetaProvider`), а не из
`rec.meta_provider`. Поэтому `TryFrom<&str>` для `meta_provider: String` из §5 дока в этом
рефакторинге **не нужен** — вопрос откладывается.

---

## Дерево до / после

```
instance/download/                          instance/download/
├── mod.rs                                  ├── mod.rs
├── manifest.rs                             ├── manifest.rs
├── libs/                                   ├── sync.rs              ← НОВЫЙ (ядро)
│   ├── mod.rs                              ├── libs/
│   ├── download.rs        ← УДАЛИТЬ        │   ├── mod.rs           (переписан)
│   ├── register.rs        ← УДАЛИТЬ        │   ├── storage.rs       ← НОВЫЙ
│   └── parse/                              │   └── parse/
│       ├── mod.rs                          │       ├── mod.rs       (хвост)
│       └── prism.rs                        │       └── prism.rs     (хвост + requires)
└── assets/                                 └── assets/
    ├── mod.rs                                  ├── mod.rs           (переписан)
    ├── parse.rs                                ├── parse.rs         (чистая функция)
    ├── download.rs        ← УДАЛИТЬ            └── storage.rs       ← НОВЫЙ
    └── register.rs        ← УДАЛИТЬ

instance/paths/mod.rs      → instance/paths.rs  (сплющить, урезать)
```

---

## Этап 0 — разблокировать компиляцию

Сейчас `cargo check` даёт 40 ошибок, из них 19 — `unable to open database file` от макросов
`sqlx`, то есть шум окружения. Пока он не убран, «убывание числа ошибок» не работает как критерий.

### 0.1. Окружение

`.sqlx/` удалён в рабочем дереве (staged deletions в `git status`). Восстановить:

```bash
git restore --staged --worktree .sqlx
```

либо пересобрать после `sqlx migrate run`:

```bash
cargo sqlx prepare
```

`dev.db` на месте и читается через `sqlite3`, но макросы репортят SQLITE_CANTOPEN — проверьте право
на запись в файл, sqlx открывает БД в r/w.

### 0.2. `main.rs:33`

`mod operation;` ссылается на несуществующий файл — подсистема живёт в `data/registry/operation/`.
Удалить строку.

### 0.3. `instance/paths/mod.rs` → `instance/paths.rs`

Снимает E0560 ×3 (`paths/mod.rs:22,23,31`) и даёт каталог natives, который нужен на этапе 5.

```rust
use std::path::PathBuf;

use crate::data::layout::LauncherPaths;

#[derive(Debug, Clone)]
pub struct InstancePaths {
    root: PathBuf,
    natives: PathBuf,
}

impl InstancePaths {
    pub fn resolve(launcher: &LauncherPaths, dir: &str) -> Self {
        let root = launcher.instances().join(dir);
        let natives = root.join("natives");

        Self { root, natives }
    }

    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    pub fn natives(&self) -> &PathBuf {
        &self.natives
    }
}
```

Легаси-поля (`instance_manifest_file`, `instances_list_file`, `headers`, `metacache_file`, `meta`) и
`set_version_manifest_file` удалить — геттеры не вызываются нигде. Общие каталоги
(`libraries`, `assets`) остаются за `LauncherPaths`, дублировать их в `InstancePaths` не нужно.

### 0.4. Мёртвые импорты и рекурсия

- `libs/download.rs:8`, `assets/download.rs:5`, `assets/mod.rs:14` — эти файлы всё равно удаляются
  на этапах 2–3, но чтобы `cargo check` был читаем уже сейчас, замените `crate::websocket::...` на
  `crate::data::registry::operation::message::{target,stage}`.
- `assets/mod.rs:43-45` — `Downloadable::get_hash` вызывает `self.get_hash()`, то есть сам себя.
  Спасает только приоритет inherent-метода (`assets/mod.rs:109`). Вернуть `&self.hash`, inherent
  `get_hash` удалить.
- `manifest.rs` — удалить мёртвые `is_asset_downloaded` (:74, всегда возвращает `true`, тело —
  отладочные `println!`) и `is_array_exists` (:96).

**Проверка:** ошибок E0433/E0560 и по `instance/paths` нет.

---

## Этап 1 — ядро: `instance/download/sync.rs`

Новый файл. Общий синхронизатор поверх уже существующего `Downloadable`
(`utils/download/mod.rs:39`), который **оба** типа уже реализуют. Основа готова, надо снять с неё
копии.

```rust
use std::{collections::HashSet, path::PathBuf, sync::Arc};

use async_std::task;
use futures::{stream::FuturesUnordered, StreamExt};
use thiserror::Error;
use tide::utils::async_trait;

use crate::{
    data::{
        db::DbError,
        registry::operation::{
            handle::OperationHandle,
            message::{
                stage::OperationStage,
                target::{FileStatus, ProcessTarget},
            },
        },
    },
    utils::download::{buffer::BufferPool, Download, Downloadable},
};

/// Единственный экземпляр на проект. Сейчас константы продублированы в
/// libs/download.rs:13-14 и assets/parse.rs:10-11, причём копии уже
/// разошлись: в assets стоит литерал 100 вместо собственной константы.
pub const CONCURRENT_TASKS_COUNT: usize = 100;
pub const CONCURRENT_BUFFERS_SIZE: usize = 16 * 1024; // 16 KiB each

#[derive(Debug, Error)]
pub enum SyncError {
    #[error(transparent)]
    Store(#[from] DbError),

    #[error("{failed} of {total} downloads failed; first error: {first}")]
    DownloadsFailed {
        failed: usize,
        total: usize,
        first: String,
    },
}

/// Что модуль умеет хранить между запусками. Реализуют libs и assets по-своему.
#[async_trait]
pub trait SyncStore<T>: Send + Sync {
    /// Хэши из `wanted`, которые уже зарегистрированы.
    /// Реализация обязана спрашивать `WHERE hash IN (...)` чанками,
    /// а не выгружать таблицу целиком.
    async fn cached(&self, wanted: &[T]) -> Result<HashSet<String>, DbError>;

    /// Батч-вставка. Идемпотентна: повторный вызов с теми же элементами
    /// не создаёт дубликатов.
    async fn register(&self, items: &[T]) -> Result<(), DbError>;
}

#[derive(Debug)]
pub struct SyncOutcome<T> {
    /// Уже были на диске и в БД
    pub cached: Vec<T>,
    /// Скачаны в этом прогоне
    pub downloaded: Vec<T>,
}

impl<T> SyncOutcome<T> {
    /// Единый проход по всему, что доступно локально.
    /// Именно он заменяет четыре разных места сборки classpath:
    /// «нашли в кэше» и «скачали» перестают быть разными ветками кода.
    pub fn all(&self) -> impl Iterator<Item = &T> {
        self.cached.iter().chain(self.downloaded.iter())
    }
}

pub struct Syncer {
    concurrency: usize,
    buffers: Arc<BufferPool>,
}

impl Default for Syncer {
    fn default() -> Self {
        Self::new(CONCURRENT_TASKS_COUNT, CONCURRENT_BUFFERS_SIZE)
    }
}

impl Syncer {
    pub fn new(concurrency: usize, buffer_size: usize) -> Self {
        Self {
            concurrency,
            buffers: Arc::new(BufferPool::new(concurrency, buffer_size)),
        }
    }

    pub async fn run<T, F>(
        &self,
        wanted: Vec<T>,
        save_path: F,
        store: &dyn SyncStore<T>,
        op: &OperationHandle,
        stage: OperationStage,
    ) -> Result<SyncOutcome<T>, SyncError>
    where
        T: Downloadable + Send + Sync + 'static,
        F: Fn(&T) -> PathBuf,
    {
        let wanted = dedup_by_hash(wanted);
        let total = wanted.len();

        let known = store.cached(&wanted).await?;
        let (cached, missing): (Vec<T>, Vec<T>) = wanted
            .into_iter()
            .partition(|item| known.contains(item.get_hash()));

        let cached_count = cached.len();

        // Одна система координат с самого начала: при полном кэше первое же
        // событие показывает 100%, а не «0 из 500». Сейчас current берётся из
        // downloaded.len() (только эта сессия), а total — из всего нужного,
        // поэтому при частично заполненном кэше 100% недостижимы.
        op.update_stage(stage.clone(), cached_count, total, None);

        let mut downloaded: Vec<T> = Vec::with_capacity(missing.len());
        let mut failures: Vec<String> = Vec::new();
        let mut futures = FuturesUnordered::new();

        for item in missing {
            let download = Download::new(save_path(&item), item, Arc::clone(&self.buffers));
            futures.push(task::spawn(download.download_with_checksum()));

            // Ждём ОДНУ завершённую задачу, а не осушаем очередь целиком.
            // Текущий process_futures (libs/download.rs:136, assets/download.rs:17)
            // дренирует FuturesUnordered до конца, создавая барьер каждые 100 файлов.
            if futures.len() >= self.concurrency {
                if let Some(res) = futures.next().await {
                    self.absorb(res, &mut downloaded, &mut failures, op, &stage, cached_count, total);
                }
            }
        }

        while let Some(res) = futures.next().await {
            self.absorb(res, &mut downloaded, &mut failures, op, &stage, cached_count, total);
        }

        // Регистрируем успешные ДО возврата ошибки: иначе повторная попытка
        // установки качает всё заново.
        if !downloaded.is_empty() {
            store.register(&downloaded).await?;
        }

        if let Some(first) = failures.first() {
            return Err(SyncError::DownloadsFailed {
                failed: failures.len(),
                total,
                first: first.clone(),
            });
        }

        Ok(SyncOutcome { cached, downloaded })
    }

    fn absorb<T: Downloadable>(
        &self,
        result: Result<T, String>,
        downloaded: &mut Vec<T>,
        failures: &mut Vec<String>,
        op: &OperationHandle,
        stage: &OperationStage,
        cached_count: usize,
        total: usize,
    ) {
        match result {
            Ok(item) => {
                let name = item.get_name().clone();
                downloaded.push(item);

                op.update_stage(
                    stage.clone(),
                    cached_count + downloaded.len(),
                    total,
                    Some(ProcessTarget::file(name, FileStatus::Downloaded)),
                );
            }
            // Сейчас здесь `println!("{e}"); None` (libs/download.rs:66,
            // assets/parse.rs:68): упавшая библиотека молча исчезает, инстанс
            // собирается неполным и падает при запуске с невнятной ошибкой Java.
            Err(e) => failures.push(e),
        }
    }
}

/// Один и тот же артефакт приходит из нескольких веток манифеста.
/// Сейчас дедупа нет нигде: downloadable_libs — Vec, и одинаковый хэш
/// даёт две параллельные загрузки в один файл.
fn dedup_by_hash<T: Downloadable>(items: Vec<T>) -> Vec<T> {
    let mut seen: HashSet<String> = HashSet::new();

    items
        .into_iter()
        .filter(|item| seen.insert(item.get_hash().clone()))
        .collect()
}
```

Три решения, зафиксированные именно здесь:

**`save_path` — замыкание, а не метод трейта.** Это единственное реальное различие между libs и
assets: библиотеки берут путь из maven-координат, ассеты — из первых двух символов хэша. Замыкание
оставляет различие у вызывающего и не заставляет `Downloadable` знать про раскладку каталогов.

**`SyncOutcome` отдаёт `cached` и `downloaded` раздельно, но однородно.** Баг с разделителями
исчезает не потому, что его починили, а потому, что исчезает место, где он возникал.

**Регистрация до ошибки.** `store.register` вызывается для успешных даже когда часть упала, и
только потом возвращается `Err`. Это делает повторную установку дешёвой.

Зарегистрировать модуль в `instance/download/mod.rs`:

```rust
pub mod assets;
pub mod libs;
pub mod manifest;
pub mod sync;
```

---

## Этап 2 — libs

### 2.1. `libs/storage.rs` (новый)

По образцу `java/storage.rs` (`JavaStore` / `DbJavaStore`). Весь sqlx из `libs/download.rs` и
`libs/register.rs` уезжает сюда — SQL не место в файле с именем `download.rs`.

```rust
use std::{collections::HashSet, sync::Arc};

use sqlx::{QueryBuilder, Sqlite};
use tide::utils::async_trait;

use crate::{
    data::db::{Database, DbError},
    instance::download::{libs::LibInfo, sync::SyncStore},
};

/// У SQLite есть предел числа переменных в запросе
/// (SQLITE_MAX_VARIABLE_NUMBER, исторически 999).
const HASH_CHUNK: usize = 500;
/// 5 колонок на строку → 999 / 5 ≈ 199, берём с запасом.
const INSERT_CHUNK: usize = 150;

pub struct DbLibStore {
    db: Arc<Database>,
}

impl DbLibStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl SyncStore<LibInfo> for DbLibStore {
    /// Было: SELECT всей таблицы + фильтр в памяти (libs/download.rs:32),
    /// причём на каждом уровне рекурсии по `requires`.
    async fn cached(&self, wanted: &[LibInfo]) -> Result<HashSet<String>, DbError> {
        let mut found = HashSet::with_capacity(wanted.len());

        for chunk in wanted.chunks(HASH_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> =
                QueryBuilder::new("SELECT hash FROM libraries WHERE hash IN (");

            let mut list = qb.separated(", ");
            for lib in chunk {
                list.push_bind(lib.hash().clone());
            }
            qb.push(")");

            let hashes: Vec<String> = qb.build_query_scalar().fetch_all(&self.db.pool).await?;
            found.extend(hashes);
        }

        Ok(found)
    }

    /// Было: INSERT ... RETURNING id через fetch_one в цикле (libs/register.rs:22).
    async fn register(&self, items: &[LibInfo]) -> Result<(), DbError> {
        for chunk in items.chunks(INSERT_CHUNK) {
            let mut qb: QueryBuilder<Sqlite> =
                QueryBuilder::new("INSERT INTO libraries (name, hash, path, native, url) ");

            qb.push_values(chunk, |mut row, lib| {
                row.push_bind(lib.name().clone())
                    .push_bind(lib.hash().clone())
                    .push_bind(lib.path().clone())
                    .push_bind(lib.is_native())
                    .push_bind(lib.url().clone());
            });

            // Требует UNIQUE(hash). Уже есть: ограничение внесено
            // в 0001_init.sql (см. этап 6), БД пересоздана.
            qb.push(" ON CONFLICT(hash) DO NOTHING");

            qb.build().execute(&self.db.pool).await?;
        }

        Ok(())
    }
}
```

`?` работает: `From<sqlx::Error> for DbError` уже есть (`data/db/mod.rs:80`).

### 2.2. `libs/parse/mod.rs` и `libs/parse/prism.rs`

Меняются **только начало и конец** функций. Логика разбора — правила OS, классификаторы,
`mainJar` — не трогается.

`parse/mod.rs`, хвост `parse_manifest_official`:

```rust
// было:
//     pub async fn parse_manifest_official(&self) -> Result<SyncResult, String>
//     ...
//     Self::download_missing_libs(downloadable_libs, &self.db).await

pub async fn parse_manifest_official(&self) -> Result<Vec<LibInfo>, LibsSyncError> {
    // ... тело без изменений ...
    Ok(downloadable_libs)
}
```

Заодно `parse/mod.rs:94` и `:97-99` — `self.manifest["id"].as_str().unwrap()` и
`downloads.client.sha1` unwrap: заменить на `and_then(...)` с пропуском клиентского jar, если
полей нет. Паника на кривом манифесте здесь ничем не оправдана.

`parse/prism.rs` — блок `requires` (:113-151) и хвост (:187-194):

```rust
pub async fn parse_manifest_prism(&self) -> Result<Vec<LibInfo>, LibsSyncError> {
    let mut downloadable_libs: Vec<LibInfo> = Vec::new();

    // ... блок разбора `libraries` без изменений ...

    // Рекурсия по `requires`: собираем в ТОТ ЖЕ Vec.
    // Ушли: additional_classpaths, additional_natives_paths и
    // result.classpaths[0].insert(0, ':') — четвёртое место сборки classpath.
    if let Some(requires) = self.manifest.get("requires").and_then(|v| v.as_array()) {
        // Было: собственный META_BASE_URL (prism.rs:15), четвёртая копия
        // базового URL в проекте. base_url() отдаёт строку БЕЗ хвостового
        // слэша, поэтому формат меняется с "{}{}/{}.json" на "{}/{}/{}.json".
        let base_url = self.provider.source().base_url();

        for req in requires {
            if let (Some(suggests), Some(uid)) = (
                req.get("suggests").and_then(|v| v.as_str()),
                req.get("uid").and_then(|v| v.as_str()),
            ) {
                let url = format!("{}/{}/{}.json", base_url, uid, suggests);

                let manifest = download_manifest::<String>(&url, None)
                    .await
                    .map_err(|e| LibsSyncError::ManifestMalformed(e.to_string()))?
                    .0;

                let child = LibsData {
                    manifest: &manifest,
                    paths: self.paths,
                    provider: self.provider,
                    current_os: self.current_os,
                };

                downloadable_libs.extend(Box::pin(child.parse_manifest_prism()).await?);
            }
        }
    }

    // ... блок mainJar без изменений, кроме self.paths.libs() → self.paths.libraries() (prism.rs:172) ...

    Ok(downloadable_libs)
}
```

Что это чинит:

- **Загрузки параллелятся между уровнями.** У Fabric-инстанса цепочка `requires` —
  `fabric-loader → intermediary → net.minecraft → org.lwjgl3`, четыре последовательные пачки
  вместо одной. При `CONCURRENT_TASKS_COUNT = 100` пул почти всё время простаивал.
- **Прогресс перестаёт откатываться.** `download_missing_libs` брал
  `downloadable_libs_count = downloadable_libs.len()` своего уровня (`libs/download.rs:21`), и UI
  видел `0/12 → 12/12`, потом `0/40 → 40/40`.
- **Один SELECT вместо N.** Каждый уровень делал свой полный скан `libraries`.
- **Дедуп становится явным.** Сейчас межуровневый дедуп работает только потому, что дочерний
  уровень успевает записаться в БД до `SELECT` родителя, — то есть БД используется как механизм
  дедупа посреди операции, и распараллелить уровни нельзя без двойных загрузок.
- **Парсер становится тестируемым без БД.** Сейчас он возвращает `SyncResult` — тип загрузчика — и
  поэтому обязан таскать `db`.

### 2.3. `libs/mod.rs` (переписан)

```rust
use std::{path::PathBuf, sync::Arc, time::Instant};

use getset::Getters;
use serde::Deserialize;
use thiserror::Error;

use crate::{
    data::{
        db::{Database, DbError},
        layout::LauncherPaths,
        registry::operation::{
            handle::OperationHandle,
            message::{
                stage::{OperationStage, StageResult},
                status::Outcome,
            },
        },
    },
    instance::download::sync::{SyncError, Syncer},
    utils::{download::Downloadable, maven},
    version::provider::MetaProvider,
};

mod parse;
mod storage;

use storage::DbLibStore;

const STAGE_TYPE: OperationStage = OperationStage::DownloadLibs;

#[derive(Error, Debug)]
pub enum LibsSyncError {
    #[error("OS is not supported")]
    OsNotAvailable,

    #[error("CPU architecture is not supported")]
    ArchNotAvailable,

    #[error("failed to parse manifest: {0}")]
    ManifestMalformed(String),

    #[error(transparent)]
    Sync(#[from] SyncError),

    #[error(transparent)]
    Db(#[from] DbError),
}

#[derive(Debug)]
pub struct LibsOutcome {
    /// Vec<PathBuf>, а не Vec<String> с разделителями внутри
    pub classpath: Vec<PathBuf>,
    pub natives: Vec<PathBuf>,
}

/// Контекст разбора манифеста. Ни `db`, ни загрузок — только то,
/// что нужно парсеру. Лайфтайм 'b и Arc<&InstancePaths> ушли.
pub struct LibsData<'a> {
    manifest: &'a serde_json::Value,
    paths: &'a LauncherPaths,
    provider: MetaProvider,
    current_os: &'static str,
}

#[derive(Eq, Hash, PartialEq, Debug, Clone, Deserialize, sqlx::FromRow, Getters)]
pub struct LibInfo {
    #[get = "pub"]
    hash: String,
    #[get = "pub"]
    name: String,
    /// Абсолютный путь на диске, собранный build_maven_file_path.
    /// Геттер новый — нужен замыканию save_path.
    #[get = "pub"]
    path: String,
    #[get = "pub"]
    url: String,
    native: bool,
}

impl Downloadable for LibInfo {
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

impl LibInfo {
    pub fn is_native(&self) -> bool {
        self.native
    }
}

impl<'a> LibsData<'a> {
    pub async fn sync(
        manifest: &'a serde_json::Value,
        paths: &'a LauncherPaths,
        provider: MetaProvider,
        db: Arc<Database>,
        op: &OperationHandle,
    ) -> Result<LibsOutcome, LibsSyncError> {
        op.start_stage(STAGE_TYPE);
        let started = Instant::now();

        let data = LibsData {
            manifest,
            paths,
            provider,
            current_os: construct_os_name()?,
        };

        // 1. Парсинг: чистый обход манифеста, включая рекурсию по `requires`.
        //    Ни БД, ни файлов — только сеть для под-манифестов.
        let wanted = match provider {
            MetaProvider::Mojang => data.parse_manifest_official().await?,
            MetaProvider::Prism => data.parse_manifest_prism().await?,
        };

        // 2. Синхронизация: один Syncer на ВЕСЬ набор, а не на каждый уровень.
        //    Политика провала: непустой failed → Err (зафиксировано).
        let store = DbLibStore::new(db);
        let outcome = Syncer::default()
            .run(
                wanted,
                |lib| PathBuf::from(lib.path()),
                &store,
                op,
                STAGE_TYPE,
            )
            .await?;

        // 3. Один проход по cached + downloaded. Именно здесь исчезает баг
        //    с разделителями: было четыре места сборки classpath с разными
        //    правилами (libs/download.rs:95, libs/register.rs:38,
        //    parse/prism.rs:140, LaunchInfoBuilder::build).
        let mut classpath = Vec::with_capacity(outcome.cached.len() + outcome.downloaded.len());
        let mut natives = Vec::new();

        for lib in outcome.all() {
            let path = PathBuf::from(lib.path());

            if lib.is_native() {
                natives.push(path.clone());
            }
            classpath.push(path);
        }

        op.complete_stage(StageResult {
            status: Outcome::Completed,
            stage: STAGE_TYPE,
            duration_secs: started.elapsed().as_secs_f64(),
            error: None,
        });

        Ok(LibsOutcome { classpath, natives })
    }

    pub fn build_maven_file_path(&self, maven_path: &str) -> String {
        // было self.paths.libs() — такого метода у LauncherPaths нет
        maven::build_file_path(self.paths.libraries(), maven_path)
    }
}

// construct_os_name() — без изменений (libs/mod.rs:126-169)
```

### 2.4. Удалить

`libs/download.rs` и `libs/register.rs` целиком. Вместе с ними уходят `add_to_classpaths`
(`download.rs:95`), `get_missing_by_hash` (`:104`), `process_futures` (`:127`) и `SyncResult`.

---

## Этап 3 — assets

### 3.1. `assets/parse.rs` (чистая функция)

```rust
use crate::instance::download::assets::{AssetInfo, AssetSyncError};

const ASSETS_BASE_URL: &str = "https://resources.download.minecraft.net";

/// Чистая функция: манифест → список ассетов. Ни БД, ни загрузок.
/// Раньше здесь же жили SELECT всей таблицы, спавн задач, пул буферов
/// и вызов register_assets.
pub fn extract_assets(manifest: &serde_json::Value) -> Result<Vec<AssetInfo>, AssetSyncError> {
    let objects = manifest["objects"]
        .as_object()
        .ok_or(AssetSyncError::ManifestMalformed)?;

    let mut assets = Vec::with_capacity(objects.len());

    for (name, value) in objects {
        // len() >= 2 обязателен: путь и URL режут hash[..2],
        // на коротком хэше это паника
        let hash = match value["hash"].as_str() {
            Some(h) if h.len() >= 2 => h,
            _ => continue,
        };

        assets.push(AssetInfo::new(
            name.clone(),
            hash.to_string(),
            format!("{}/{}/{}", ASSETS_BASE_URL, &hash[..2], hash),
        ));
    }

    Ok(assets)
}
```

Константы `CONCURRENT_*` (`parse.rs:10-11`) и литерал `100` (`parse.rs:74`) уходят — они в
`sync.rs`. `construct_asset_url` схлопывается сюда: относительный путь сохранения теперь даёт
замыкание `save_path`.

### 3.2. `assets/storage.rs` (новый)

Копия `libs/storage.rs` по форме, таблица `assets (name, hash, url)`:

```rust
const HASH_CHUNK: usize = 500;
/// 3 колонки на строку
const INSERT_CHUNK: usize = 300;

pub struct DbAssetStore { db: Arc<Database> }

#[async_trait]
impl SyncStore<AssetInfo> for DbAssetStore {
    async fn cached(&self, wanted: &[AssetInfo]) -> Result<HashSet<String>, DbError> {
        // SELECT hash FROM assets WHERE hash IN (...) чанками
    }

    async fn register(&self, items: &[AssetInfo]) -> Result<(), DbError> {
        // INSERT INTO assets (name, hash, url) VALUES ... ON CONFLICT(hash) DO NOTHING
    }
}
```

Таблица `assets` — самый весомый аргумент за `WHERE hash IN`: у современной версии там несколько
тысяч строк, и сейчас она целиком выгружается в память на каждую установку (`assets/parse.rs:25`).

### 3.3. `assets/mod.rs` (переписан)

```rust
use std::{path::PathBuf, sync::Arc, time::Instant};

use getset::Getters;
use thiserror::Error;

use crate::{
    data::{
        db::{Database, DbError},
        layout::LauncherPaths,
        registry::operation::{
            handle::OperationHandle,
            message::{
                stage::{OperationStage, StageResult},
                status::Outcome,
            },
        },
    },
    instance::download::sync::{SyncError, Syncer},
    utils::download::Downloadable,
};

mod parse;
mod storage;

use storage::DbAssetStore;

const STAGE_TYPE: OperationStage = OperationStage::DownloadAssets;

#[derive(Debug, Error)]
pub enum AssetSyncError {
    #[error("assets manifest has no `objects` map")]
    ManifestMalformed,

    #[error(transparent)]
    Sync(#[from] SyncError),

    #[error(transparent)]
    Db(#[from] DbError),
}

/// Ручные PartialEq/Eq/Hash по одному полю и AssetInfo::with_hash
/// больше не нужны: Syncer дедуплицирует по строке хэша, а cached()
/// возвращает HashSet<String>, а не HashSet<AssetInfo>.
#[derive(Debug, Clone, Getters)]
pub struct AssetInfo {
    #[get = "pub"]
    name: String,
    #[get = "pub"]
    hash: String,
    #[get = "pub"]
    url: String,
}

impl AssetInfo {
    pub fn new(name: String, hash: String, url: String) -> Self {
        Self { name, hash, url }
    }
}

impl Downloadable for AssetInfo {
    fn get_name(&self) -> &String {
        &self.name
    }

    /// Было `self.get_hash()` — вызов самого себя. Спасал только
    /// приоритет inherent-метода; любой перенос метода в трейт
    /// превращал это в бесконечную рекурсию.
    fn get_hash(&self) -> &String {
        &self.hash
    }

    fn get_url(&self) -> &String {
        &self.url
    }
}

#[derive(Debug)]
pub struct AssetsOutcome {
    pub total: usize,
    pub downloaded: usize,
}

pub struct AssetsData;

impl AssetsData {
    /// Было: возвращает (), глотает ошибку через println! (assets/mod.rs:77-80)
    pub async fn sync(
        manifest: &serde_json::Value,
        paths: &LauncherPaths,
        db: Arc<Database>,
        op: &OperationHandle,
    ) -> Result<AssetsOutcome, AssetSyncError> {
        op.start_stage(STAGE_TYPE);
        let started = Instant::now();

        let objects_dir = paths.assets().join("objects");
        let wanted = parse::extract_assets(manifest)?;

        let store = DbAssetStore::new(db);
        let outcome = Syncer::default()
            .run(
                wanted,
                |asset| {
                    let hash = asset.hash();
                    objects_dir.join(&hash[..2]).join(hash)
                },
                &store,
                op,
                STAGE_TYPE,
            )
            .await?;

        op.complete_stage(StageResult {
            status: Outcome::Completed,
            stage: STAGE_TYPE,
            duration_secs: started.elapsed().as_secs_f64(),
            error: None,
        });

        Ok(AssetsOutcome {
            total: outcome.cached.len() + outcome.downloaded.len(),
            downloaded: outcome.downloaded.len(),
        })
    }
}
```

### 3.4. Удалить

`assets/download.rs` (там только дублирующий `process_futures`) и `assets/register.rs`.

**Проверка этапов 2–3:**

```bash
grep -rn "CONCURRENT_TASKS_COUNT\|CONCURRENT_BUFFERS_SIZE\|process_futures" src/  # по одному
grep -rn "crate::websocket" src/                                                  # пусто
grep -rn "add_to_classpaths\|insert(0, ':')" src/                                 # пусто
```

---

## Этап 4 — `init.rs` и `service.rs`

### 4.1. `manifest.rs` — сигнатура `get_assets_manifest`

Принимать `&Path`, а не `&str`, чтобы убрать `.to_str().unwrap()` на стороне вызова
(`init.rs:65`):

```rust
pub async fn get_assets_manifest<'a, P: AsRef<Path>>(
    version_manifest: &'a serde_json::Value,
    save_dir: P,
) -> Result<(serde_json::Value, &'a str), DownloadError>
```

и `asset_index["id"].as_str().unwrap()` (`manifest.rs:65`) → `ok_or(...)`.

### 4.2. `instance/init.rs`

```rust
use std::{sync::Arc, time::Instant};

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
        download::{
            assets::AssetsData,
            libs::LibsData,
            manifest::{download_manifest, get_assets_manifest},
        },
        error::InstanceError,
        launch::{args::ArgType, LaunchInfo, LaunchInfoBuilder},
        model::InstanceRecord,
        paths::InstancePaths,
        Result,
    },
    version::provider::VersionSummary,
};

pub async fn install(
    version: VersionSummary,
    rec: &InstanceRecord,
    paths: &LauncherPaths,
    db: Arc<Database>,
    op: &OperationHandle,
) -> Result<LaunchInfo> {
    let instance_paths = InstancePaths::resolve(paths, &rec.dir);

    let mut launch_builder = LaunchInfoBuilder::new();
    // было paths.instances().join(rec.name): `dir` — это slugified-имя,
    // под которым каталог реально создаётся (service.rs:53); `name` —
    // отображаемое имя, и оно к тому же не Copy
    launch_builder.set_arg_value(ArgType::GameDir, instance_paths.root());
    launch_builder.set_arg_value(ArgType::AssetsDir, paths.assets());
    launch_builder.set_natives_dir(instance_paths.natives().clone());

    // --- Стадия 1: манифест версии ---
    op.start_stage(OperationStage::FetchManifest);
    let started = Instant::now();

    let (version_manifest, _) = download_manifest(&version.url, Some(paths.meta()))
        .await
        .map_err(|e| {
            InstanceError::CreationFailed(format!("failed to download version manifest: {e}"))
        })?;

    op.complete_stage(StageResult {
        status: Outcome::Completed,
        stage: OperationStage::FetchManifest,
        duration_secs: started.elapsed().as_secs_f64(),
        error: None,
    });

    launch_builder.add_version(&version.id);

    // --- Стадия 2: libs (start/complete внутри LibsData::sync) ---
    let libs = LibsData::sync(
        &version_manifest,
        paths,
        version.provider,
        Arc::clone(&db),
        op,
    )
    .await
    .map_err(|e| InstanceError::CreationFailed(format!("failed to sync libs: {e}")))?;

    launch_builder.add_cps(libs.classpath);
    launch_builder.add_natives(libs.natives);

    // --- Манифест ассетов ---
    let (assets_manifest, asset_index) =
        get_assets_manifest(&version_manifest, paths.assets().join("indexes"))
            .await
            .map_err(|e| {
                InstanceError::CreationFailed(format!("failed to download assets manifest: {e}"))
            })?;

    launch_builder.set_arg_value(ArgType::AssetIndex, asset_index);

    // --- Стадия 3: assets ---
    AssetsData::sync(&assets_manifest, paths, db, op)
        .await
        .map_err(|e| InstanceError::CreationFailed(format!("failed to sync assets: {e}")))?;

    launch_builder
        .fill_defaults()
        .build()
        .map_err(|e| InstanceError::CreationFailed(e.to_string()))
}
```

Что ушло относительно текущего кода:

- `Arc::clone(&state)` (`init.rs:83`) — домену нужны `db`, `paths`, `op`, а не контейнер целиком.
  То же правило уже применено в `java`, и оно же делает библиотечную сборку возможной;
- `.to_str().unwrap()` на пути к indexes (`init.rs:65`);
- `println!("{:#?}", result)` (`init.rs:50`);
- `LibsData::get_classpaths_mut` / `take_natives_paths` — костыльные аксессоры к `SyncResult`.

### 4.3. `instance/service.rs`

```rust
pub struct InstanceService {
    db: Arc<Database>,          // ← новое поле: сейчас new() принимает db, но не сохраняет
    registry: Arc<InstanceRegistry>,
    operations: Arc<OperationRegistry>,
    paths: Arc<LauncherPaths>,
    bus: EventBus,
}

impl InstanceService {
    pub fn new(
        db: Arc<Database>,
        operations: Arc<OperationRegistry>,
        paths: Arc<LauncherPaths>,
        bus: EventBus,
    ) -> Self {
        Self {
            registry: Arc::new(InstanceRegistry::new(Arc::clone(&db))),
            db,
            operations,
            paths,
            bus,
        }
    }

    pub async fn install(&self, version: VersionSummary, id: InstanceId) -> Result<LaunchInfo> {
        let rec = self.get_instance(id).await?;

        let op = self.operations.begin(
            "instance.install".to_string(),
            vec![
                OperationStage::FetchManifest,
                OperationStage::DownloadLibs,
                OperationStage::DownloadAssets,
            ],
        );

        // Было: вызов без `?` и с `;` — LaunchInfo терялся, ошибка игнорировалась,
        // op.finish не вызывался никогда, и снапшот навсегда оставался
        // в OperationRegistry.active
        match instance::init::install(version, &rec, &self.paths, Arc::clone(&self.db), &op).await {
            Ok(info) => {
                op.finish(Outcome::Completed);
                Ok(info)
            }
            Err(e) => {
                op.finish(Outcome::Failed);
                Err(e)
            }
        }
    }
}
```

---

## Этап 5 — classpath до конца (`launch/`)

Без этого этапа смысл рефакторинга теряется: `SyncOutcome` отдаст корректные пути, а
`LaunchInfoBuilder::build` (`launch/mod.rs:134-137`) снова склеит их `push_str`-ом без
разделителей.

```rust
use std::{env, ffi::OsString, path::PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("classpath contains the platform path separator: {0}")]
    ClasspathSeparator(String),

    #[error("classpath is not valid unicode")]
    ClasspathNotUnicode,

    #[error("no java runtime selected")]
    JavaRuntimeMissing,
}

#[derive(Debug, Clone)]
pub struct LaunchInfo {
    classpath: String,
    native_libs: Vec<PathBuf>,
    natives_dir: PathBuf,          // ← новое
    main_class: Option<String>,
    game_args: HashMap<String, String>,
    java_bin_path: JavaRuntime,
    name: String,
}

#[derive(Debug, Default)]
pub struct LaunchInfoBuilder {
    classpath: Vec<PathBuf>,       // ← было Vec<String>
    native_libs: Vec<PathBuf>,
    natives_dir: Option<PathBuf>,
    // ... остальное как есть
}

impl LaunchInfoBuilder {
    pub fn add_cps<I: IntoIterator<Item = PathBuf>>(&mut self, iter: I) -> &mut Self {
        self.classpath.extend(iter);
        self
    }

    pub fn set_natives_dir(&mut self, dir: PathBuf) -> &mut Self {
        self.natives_dir = Some(dir);
        self
    }

    /// Опечатка fill_defauls исправлена (launch/mod.rs:116)
    pub fn fill_defaults(mut self) -> Self { /* без изменений */ }

    pub fn build(self) -> Result<LaunchInfo, LaunchError> {
        // env::join_paths сам подставит ':' или ';' по платформе и вернёт
        // ошибку, если путь содержит разделитель. Разделитель был захардкожен
        // как ':' в четырёх местах — на Windows нужен ';'.
        let classpath: OsString = env::join_paths(&self.classpath)
            .map_err(|e| LaunchError::ClasspathSeparator(e.to_string()))?;

        let classpath = classpath
            .into_string()
            .map_err(|_| LaunchError::ClasspathNotUnicode)?;

        // Было todo!() — паника на отсутствующем рантайме (launch/mod.rs:147)
        let java_bin_path = self.preferred_java.ok_or(LaunchError::JavaRuntimeMissing)?;

        let natives_dir = self.natives_dir.unwrap_or_default();

        Ok(LaunchInfo { classpath, natives_dir, java_bin_path, /* ... */ })
    }
}
```

`launch/execute.rs:49` — убрать захардкоженный
`/Users/quartix/.sonata/instances/1.5.2/natives`:

```rust
if !info.native_libs.is_empty() {
    let native_dir = info.natives_dir.clone();
    match Natives::extract(info.native_libs, &native_dir).await { /* ... */ }
}
```

Существующий тест `builder_default_empty` (`launch/mod.rs:167`) сломается — `build()` теперь
`Result` и без java-рантайма вернёт `Err(JavaRuntimeMissing)`. Переписать его на проверку именно
этого, плюс новый тест на инвариант разделителей (см. ниже).

---

## Этап 6 — устойчивость (можно отдельным PR)

Таблицы `libraries` и `assets` не имели UNIQUE по `hash`. Два инстанса, качающие одну библиотеку
параллельно, давали дубликаты.

**Сделано: ограничение внесено прямо в `migrations/0001_init.sql`, отдельной миграции нет.**
Продукт ещё не роздан, БД пересоздаётся без потерь, поэтому дедуп-миграция с
`DELETE ... WHERE id NOT IN (SELECT MIN(id) ...)` и последующим `CREATE UNIQUE INDEX` не нужна —
она существует только чтобы задним числом почистить чужие БД.

```sql
CREATE TABLE IF NOT EXISTS libraries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    hash TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    native BOOL NOT NULL,
    url TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS assets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    hash TEXT NOT NULL UNIQUE,
    url TEXT NOT NULL
);
```

Применение (правка уже применённой миграции ломает контрольную сумму в `_sqlx_migrations`, поэтому
БД пересоздаётся):

```bash
rm -f dev.db && sqlx database create && sqlx migrate run
```

`UNIQUE` в объявлении колонки создаёт тот же неявный уникальный индекс, что и
`CREATE UNIQUE INDEX`, поэтому `ON CONFLICT(hash) DO NOTHING` в обоих `SyncStore::register`
работает сразу — отдельного шага «сначала индекс, потом ON CONFLICT» больше нет.

Попутно там же исправлено и проверено:

- **Порядок миграций.** Было `0002_instances` → `0003_java_runtimes`, то есть FK объявлялся
  раньше своей цели. Стало `0002_java_runtimes` → `0003_instances`.
- **Тип FK.** `instances_settings.java_runtime TEXT` против `java_runtimes.id INTEGER` →
  `java_runtime INTEGER REFERENCES java_runtimes(id) ON DELETE SET NULL`.
- **`export_type TEXT NOT NULL DEFAULT "Sonata"`** — в SQLite двойные кавычки означают
  идентификатор, и строкой это становится только по fallback-правилу. Заменено на `'Sonata'`.
- **`PRAGMA foreign_keys = ON` в `data/db/mod.rs`** удалён: это per-connection прагма, а
  выполнялась она один раз по пулу, то есть накрывала одно соединение из нескольких.
  Реально FK работают потому, что `sqlx-sqlite` ставит эту прагму на каждое открываемое
  соединение по умолчанию (`sqlx-sqlite-0.8.6/src/options/mod.rs:185`).

Проверено на живой БД: битый FK даёт `FOREIGN KEY constraint failed`, удаление рантайма
обнуляет `instances_settings.java_runtime`, удаление инстанса каскадит, дефолт `export_type`
читается как `Sonata`.

Остаётся не починенным `instance/options/pages/settings.rs` — он относится к легаси-срезу
`options/*`, который в этом рефакторинге вне зоны работ. Там три отдельные проблемы:
`SELECT dir FROM instances_settings` (колонки `dir` в этой таблице никогда не было — она в
`instances`), `SettingsRow.java_runtime: Option<String>` против новой `INTEGER`, и
`req.state().static_data` (поля `static_data` у `GlobalState` больше нет). Вторая требует
решения по API: `SettingsFields.java_bin_path` — это путь, а в БД теперь id, значит нужен либо
`LEFT JOIN java_runtimes` на чтении и поиск по `exec_path` на записи, либо смена типа поля в
ts-биндингах.

Плюс убрать `.unwrap()` из `download_manifest` (`manifest.rs:30,36,40,42-44` — `rfind('/')`,
`File::create`, `write_all`, `to_string`); путь сохранения передавать параметром, а не выводить
нарезкой URL строкой.

---

## Тесты

### `Syncer` — `instance/download/sync.rs`, `#[cfg(test)] mod tests`

Фиктивный in-memory `SyncStore` + `httpmock` (уже в dev-dependencies):

```rust
struct FakeStore {
    known: HashSet<String>,
    registered: Mutex<Vec<String>>,
}

#[async_trait]
impl SyncStore<TestItem> for FakeStore { /* ... */ }
```

| Тест | Ожидание |
|---|---|
| `full_cache_reports_100_percent_immediately` | `downloaded` пуст, `cached.len() == wanted.len()`, первое же событие прогресса имеет `current == total` |
| `empty_cache_downloads_everything` | `wanted.len()` загрузок, `register` вызван один раз со всеми |
| `duplicates_are_downloaded_once` | `wanted` с повторяющимся хэшем → загрузок столько, сколько уникальных |
| `partial_failure_registers_successes` | один элемент отдаёт 404 → `Err(DownloadsFailed { failed: 1, .. })`, но `register` вызван с успешными |
| `progress_is_monotonic` | последовательность `current` не убывает и заканчивается на `total` |

Для проверки прогресса нужен `EventBus::subscribe()` — `OperationHandle` создаётся через
`OperationRegistry::begin`, события собираются из ресивера.

### Парсер prism — `libs/parse/prism.rs`

Стало возможно без БД. Манифест-фикстура → ожидаемый `Vec<LibInfo>`:
- правила OS: lib с `rules.action = allow` для другой ОС не попадает в результат;
- классификаторы: `natives-linux` даёт `LibInfo { native: true }`;
- `requires`: под-манифест через `httpmock`, результат содержит либы обоих уровней в одном `Vec`;
- `mainJar` даёт запись с путём `<libraries>/com/mojang/minecraft/<version>-client.jar`.

### Classpath — `launch/mod.rs`

```rust
#[test]
fn classpath_joined_with_platform_separator() {
    // N путей → ровно N-1 разделителей.
    // Сейчас инвариант нарушен (build() склеивает push_str'ом),
    // и без теста регрессия вернётся.
}

#[test]
fn build_without_java_runtime_errors() {
    // было todo!() → паника
}
```

---

## Что переиспользуем, а не пишем заново

| Уже есть | Где | Роль |
|---|---|---|
| `Downloadable` | `utils/download/mod.rs:39` | Граница `Syncer`; оба типа уже реализуют |
| `Download<T>::download_with_checksum` | `utils/download/mod.rs:54` | Загрузка с редиректами и SHA1 — не трогать |
| `BufferPool` | `utils/download/buffer.rs` | Пул буферов внутри `Syncer` |
| `OperationHandle` | `data/registry/operation/handle.rs` | `start_stage` / `update_stage` / `complete_stage` / `finish` — готов |
| `ProcessTarget::file(name, status)` | `data/registry/operation/message/process.rs:6` | Готовый конструктор для `target` |
| `LauncherPaths` | `data/layout.rs` | `libraries()`, `assets()`, `instances()`, `meta()` |
| `maven::build_file_path` | `utils/maven.rs:66` | `save_path` для libs |
| `JavaStore` / `DbJavaStore` | `java/storage.rs` | Образец для `SyncStore` + `Db*Store` |
| `DbError` | `data/db/mod.rs` | Ошибки БД, уже с `From<sqlx::Error>` |
| `VersionSource::base_url()` | `version/provider/prism.rs:42` | Замена `META_BASE_URL` |

---

## Верификация

**Компиляция.** После каждого этапа `cargo check`; смотреть на диагностики именно по
`src/instance/**`. Крейт не собирается и по независимым причинам вне зоны работ
(`instance/options/*`, `instance/mod.rs` с закомментированным легаси).

**Отсутствие дублей.**

```bash
grep -rn "CONCURRENT_TASKS_COUNT\|CONCURRENT_BUFFERS_SIZE\|process_futures" src/  # по одному
grep -rn "crate::websocket" src/                                                  # пусто
grep -rn "add_to_classpaths\|insert(0, ':')" src/                                 # пусто
grep -rn "META_BASE_URL" src/                                                     # пусто
```

**Сквозной прогон.** `InstanceService::create` + `install`:

- `~/.sonata/libraries` и `~/.sonata/assets/objects` заполняются;
- повторный `install` того же инстанса не качает ничего заново и **с первого события прогресса**
  рапортует `current == total`;
- прогресс внутри стадии `DownloadLibs` монотонно растёт и не откатывается — проверка того, что
  разделение parse/download сработало; сейчас он сбрасывается на каждом уровне `requires`;
- операция уходит из `OperationRegistry::snapshot_all()` после завершения (`op.finish` вызван);
- дублей нет: `SELECT hash, COUNT(*) FROM libraries GROUP BY hash HAVING COUNT(*) > 1` — пусто,
  то же для `assets`;
- игра запускается: `-cp` содержит разделители, natives распаковываются в
  `<instances>/<dir>/natives`, а не в захардкоженный путь.
