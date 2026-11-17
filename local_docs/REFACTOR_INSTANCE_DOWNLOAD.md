# Рефакторинг `instance/init.rs` и `instance/download/*`

Ориентиры: `TARGET_STRUCTURE.md` (целевое дерево), `FULL_ARCHITECTURE_AUDIT.md` §3.1/§6.2/§6.6,
`ANALYSIS.md` §1.3/§4.1/§4.2. Документ — план работ, код пишете вы.

---

## Context

`instance/init.rs` и `instance/download/*` — единственная часть среза `instance`, не переведённая
на новое поколение архитектуры. Вокруг них уже всё готово: `OperationRegistry`/`OperationHandle`
с `EventBus` работают, `InstanceService` вызывает `init::install`, `LauncherPaths` резолвит
раскладку, `VersionCatalog` отдаёт `VersionSummary`. А сам конвейер загрузки остался в старом
поколении: `Result<_, String>`, `serde_json::Value`, прогресс закомментирован, транспортные
импорты из удалённого модуля `websocket`.

Из-за этого `instance/` не компилируется, а `InstanceService::install` вызывает функцию, которая
ссылается на несуществующие переменные. Рефакторинг нужен, чтобы закрыть последний разрыв между
`instance` и целевой структурой и получить компилирующийся `install`-путь.

Побочная, но более важная цель: устранить главное дублирование проекта (§3.1 аудита) — два почти
одинаковых конвейера libs/assets. Третья копия появится, как только возьмётесь за скачивание
Java-рантайма.

---

## 1. Фактическое состояние

### 1.1. `init.rs` — не компилируется, разорван посередине

Функция `install(rec, paths, op) -> Result<LaunchInfo>` ссылается на переменные, которых нет в
области видимости: `client_data` (:33, :112, :113), `state` (:71, :83, :87), `register` (:85),
`instance` (:87, :98). Плюс `Self::register` (:87) в свободной функции — `Self` там не существует.

Отдельно неверная типизация: параметр объявлен как `&LauncherPaths`, но вызываются
`paths.instance()` (:27), `paths.set_version_manifest_file()` (:37) — методы `InstancePaths`,
причём второй требует `&mut`.

`InstanceService::install` (`service.rs:96`) вызывает `install` и **игнорирует возвращаемый
`LaunchInfo`** — без `?`, с `;`. То есть даже после починки результат установки теряется.

### 1.2. `InstancePaths` — конструктор не соответствует структуре

`paths/mod.rs`: поля — `instance`, `instance_manifest_file`, `instances_list_file`, `headers`,
`meta`, `version_manifest_file`, `metacache_file`. А `get_required_paths` (:20) присваивает
`libs`, `assets`, `root` — таких полей нет (E0560 ×3). Это след незаконченного переноса общих
каталогов в `LauncherPaths`.

Легаси-поля `instance_manifest_file`, `instances_list_file`, `headers`, `metacache_file` —
раскладка эпохи JSON, геттеры не вызываются нигде (§2.10 аудита).

### 1.3. Мёртвые транспортные импорты

`libs/download.rs:8` и `assets/download.rs:5` импортируют
`crate::websocket::messages::operation::process::{FileStatus, ProcessTarget}`,
`assets/mod.rs:14` — `crate::websocket::messages::operation::stage::*`. Модуль `websocket`
удалён (`main.rs:34` закомментирован) → E0433. Актуальные типы живут в
`data::registry::operation::message::{target,stage,status}`.

### 1.4. Дублирование libs ↔ assets — подтверждено построчно

| Что | libs | assets |
|---|---|---|
| `CONCURRENT_TASKS_COUNT = 100` | `download.rs:13` | `parse.rs:10` |
| `CONCURRENT_BUFFERS_SIZE = 16*1024` | `download.rs:14` | `parse.rs:11` |
| `process_futures` | `download.rs:127` | `download.rs:10` |
| Полный `SELECT` + фильтр в памяти | `download.rs:32` | `parse.rs:25` |
| `spawn` + `FuturesUnordered` + `BufferPool` | `download.rs:53-78` | `parse.rs:42-80` |
| `INSERT` в цикле | `register.rs:22` | `register.rs:11` |

Копии уже разошлись: в assets `if futures.len() >= 100` (`parse.rs:74`) — литерал вместо
собственной же константы; в libs используется константа. Классический признак копипасты.

### 1.5. Classpath собирается неправильно — из четырёх мест с разными правилами

- `add_to_classpaths` (`libs/download.rs:95`) добавляет `:` перед элементом, кроме первого;
- `register_libs` (`libs/register.rs:38`) кладёт путь **без разделителя вообще**;
- `parse/prism.rs:140` вставляет `:` в начало под-результата;
- `LaunchInfoBuilder::build` конкатенирует через `push_str` без разделителя.

Итог: свежескачанные библиотеки склеиваются в одну строку. Плюс разделитель захардкожен как `:` —
на Windows нужен `;`. Это то, что делает запуск неработоспособным (§1.3 `ANALYSIS.md`).

### 1.6. Прогресс считается в двух системах координат

`process_futures` берёт `current = downloaded.len()` (только скачанное в этой сессии), а
`total = downloadable_libs_count` / `objects.len()` (всё нужное, включая закешированное). При
частично заполненном кэше прогресс никогда не дойдёт до 100%.

### 1.7. Прочее в зоне работ

- `AssetInfo`: inherent `get_hash` (`assets/mod.rs:109`) и трейтовый `Downloadable::get_hash`
  (:43), который вызывает `self.get_hash()`. Сейчас спасает приоритет inherent-метода; любой
  перенос метода в трейт превращает это в бесконечную рекурсию.
- `sync_assets` возвращает `()` и глотает ошибку через `println!` (`assets/mod.rs:77-80`).
- `download_manifest` (`manifest.rs:27-34`) — четыре `.unwrap()`: `rfind('/')`, `File::create`,
  `write_all`, `to_string`. Путь сохранения выводится нарезкой URL строкой.
- `get_assets_manifest` (`manifest.rs:55`) — `.unwrap()` на `asset_index["id"]`.
- `is_asset_downloaded` (:64) всегда возвращает `true`, тело — отладочные `println!`;
  `is_array_exists` (:82) работает с metacache. Обе не вызываются.
- `parse/prism.rs:15` — свой `META_BASE_URL`, четвёртая копия базового URL.
- Таблицы `libraries` и `assets` (`migrations/0001_init.sql`) **не имеют UNIQUE по `hash`**,
  а `register_*` делает `INSERT ... RETURNING id` через `fetch_one` — два инстанса, качающие одну
  библиотеку параллельно, дадут дубликаты либо ошибку.

---

## 2. Целевая архитектура

### 2.1. Ядро — `download/sync.rs`

Общий синхронизатор поверх уже существующего `Downloadable` (`utils/download/mod.rs:39`), который
**оба** типа уже реализуют. Основа готова, надо снять с неё копии.

```rust
// instance/download/sync.rs

/// Что модуль умеет хранить между запусками. Реализуют libs и assets по-своему.
#[async_trait]
pub trait SyncStore<T: Downloadable>: Send + Sync {
    /// Какие из нужных хэшей уже есть. Реализация обязана спрашивать
    /// WHERE hash IN (...), а не выгружать таблицу целиком.
    async fn cached(&self, wanted: &[T]) -> Result<HashSet<String>, SyncError>;
    async fn register(&self, items: &[T]) -> Result<(), SyncError>;
}

pub struct Syncer {
    concurrency: usize,
    pool: Arc<BufferPool>,
}

pub struct SyncOutcome<T> {
    pub cached:     Vec<T>,   // уже были на диске
    pub downloaded: Vec<T>,   // скачаны сейчас
    pub failed:     Vec<(T, String)>,
}

impl Syncer {
    pub async fn run<T, F>(
        &self,
        wanted: Vec<T>,
        save_path: F,              // куда класть конкретный элемент
        store: &dyn SyncStore<T>,
        op: &OperationHandle,
        stage: OperationStage,
    ) -> Result<SyncOutcome<T>, SyncError>
    where
        T: Downloadable + Send + Sync + 'static,
        F: Fn(&T) -> PathBuf;
}
```

Три решения, которые надо принять именно здесь:

**`save_path` — замыкание, а не метод трейта.** Это единственное реальное различие между libs и
assets: библиотеки берут путь из maven-координат (`build_maven_file_path`), ассеты — из первых
двух символов хэша (`construct_asset_url`). Всё остальное идентично. Замыкание оставляет различие
на стороне вызывающего и не заставляет `Downloadable` знать про раскладку каталогов.

**`SyncOutcome` возвращает `cached` и `downloaded` раздельно, но однородно.** Сейчас classpath
собирается в двух местах разными правилами (§1.5) именно потому, что «нашли в кэше» и «скачали» —
разные ветки кода. С единым `SyncOutcome` вызывающий делает один проход по
`cached.iter().chain(downloaded.iter())` и собирает `Vec<PathBuf>`. Баг с разделителями исчезает
не потому, что его починили, а потому, что исчезает место, где он возникал.

**Прогресс — одна система координат.** Внутри `run`: `total = wanted.len()`,
`current = cached.len() + завершённые загрузки`. Стартовать `op.update_stage(stage, cached.len(),
total, None)` **до** первой загрузки — тогда при полном кэше сразу видно 100%, а не «0 из 500».

**Ошибки не глотать.** Сейчас `Err(e) => { println!("{e}"); None }` (`libs/download.rs:66`,
`assets/parse.rs:68`) — упавшая библиотека молча исчезает, инстанс собирается неполным и падает
при запуске с невнятной ошибкой Java. `Syncer` возвращает `failed`, а решение принимает
вызывающий: для libs непустой `failed` — фатально, для assets можно продолжить (игра запустится
без части звуков). Это решение стоит зафиксировать явно, а не унаследовать по умолчанию.

### 2.2. Разделение путей

`LauncherPaths` (`data/layout.rs`) уже владеет общими каталогами: `libraries()`, `assets()`,
`meta()`, `instances()`. `InstancePaths` должен владеть только тем, что относится к конкретному
инстансу, и строиться от него:

```rust
// instance/paths.rs  (сплющить каталог, см. TARGET_STRUCTURE)
pub struct InstancePaths {
    root:    PathBuf,   // launcher.instances().join(dir)
    natives: PathBuf,   // root.join("natives")
}

impl InstancePaths {
    pub fn resolve(launcher: &LauncherPaths, dir: &str) -> Self;
}
```

Легаси-поля (`instance_manifest_file`, `instances_list_file`, `headers`, `metacache_file`)
удалить. `version_manifest_file: Option<PathBuf>` с мутабельным сеттером тоже убрать — путь до
скачанного манифеста возвращает `download_manifest`, хранить его в структуре путей незачем.

Побочно это чинит `execute.rs:49` с захардкоженным `/Users/quartix/.sonata/instances/1.5.2/natives`
(§5.6 аудита): путь берётся из `paths.natives()`.

### 2.3. Сигнатуры после рефакторинга

```rust
// instance/init.rs
pub async fn install(
    rec:      &InstanceRecord,
    launcher: &LauncherPaths,
    db:       &Database,
    op:       &OperationHandle,
) -> Result<LaunchInfo>;

// instance/download/libs/mod.rs
impl LibsData<'_> {
    pub async fn sync(
        manifest: &serde_json::Value,
        paths:    &LauncherPaths,
        provider: MetaProvider,
        db:       &Database,
        op:       &OperationHandle,
    ) -> Result<LibsOutcome, LibsSyncError>;
}

pub struct LibsOutcome {
    pub classpath: Vec<PathBuf>,   // не Vec<String> с разделителями внутри
    pub natives:   Vec<PathBuf>,
}

// instance/download/assets/mod.rs
impl AssetsData<'_> {
    pub async fn sync(
        manifest: &serde_json::Value,
        paths:    &LauncherPaths,
        db:       &Database,
        op:       &OperationHandle,
    ) -> Result<AssetsOutcome, AssetSyncError>;   // не (), не println!
}
```

`Arc<GlobalState>` из сигнатур уходит: домену нужны `db`, `paths`, `op` — передавайте их, а не
контейнер целиком. Это то же правило, что уже применено в `java` (§6.1 аудита) и что делает
библиотечную сборку возможной.

### 2.4. Classpath — `Vec<PathBuf>` до самого конца

Удалить `add_to_classpaths` (`libs/download.rs:95`), `insert(0, ':')` (`parse/prism.rs:140`) и
конкатенацию в `LaunchInfoBuilder::build`. Собирать `Vec<PathBuf>`, а в момент формирования
команды запуска — один раз через `std::env::join_paths(...)`, которая сама подставит `:` или `;`
по платформе и вернёт ошибку, если путь содержит разделитель.

Это затрагивает `launch/mod.rs` (`LaunchInfo.classpaths`) — небольшой выход за границы
`download/`, но без него смысл рефакторинга теряется: `SyncOutcome` отдаст корректные пути, а
билдер снова склеит их без разделителей.

---

## 3. Поэтапный план

Крейт сейчас не компилируется (46 ошибок, из них большинство — в `instance/`). Этапы выстроены
так, чтобы число ошибок монотонно убывало и каждый этап можно было проверить `cargo check`.

### Этап 1 — разблокировать компиляцию (механика, без логики)

1. `instance/paths/mod.rs` → `instance/paths.rs`: структура по §2.2, `resolve(launcher, dir)`,
   легаси-поля удалить. Снимает E0560 ×3, E0599 `libs`.
2. Заменить импорты `crate::websocket::...` на `crate::data::registry::operation::message::...`
   в `libs/download.rs:8`, `assets/download.rs:5`, `assets/mod.rs:14`. Снимает E0433 ×5.
3. `assets/mod.rs:43` — `Downloadable::get_hash` должен возвращать `&self.hash`, а не звать
   `self.get_hash()`; inherent-метод `get_hash` (:109) удалить, оставив только трейтовый.
4. Удалить мёртвое из `manifest.rs`: `is_asset_downloaded` (:64), `is_array_exists` (:82).

**Проверка:** `cargo check` — ошибок в `instance/paths`, `assets/mod`, импортов больше нет.

### Этап 2 — `sync.rs` и перевод libs

5. Написать `download/sync.rs` по §2.1: `SyncStore`, `Syncer`, `SyncOutcome`, `SyncError`.
   Константы `CONCURRENT_TASKS_COUNT` / `CONCURRENT_BUFFERS_SIZE` — здесь, в единственном
   экземпляре (позже — в `Config`, см. §5).
6. `libs/storage.rs` — новый файл, `impl SyncStore<LibInfo>`: `cached()` с `WHERE hash IN (...)`,
   `register()` с батч-вставкой. Весь sqlx из `libs/download.rs` и `libs/register.rs` уезжает
   сюда (§6.4 аудита: SQL не место в файле с именем `download.rs`).
7. `libs/mod.rs::sync` — по сигнатуре §2.3: распарсить манифест (`parse/*` не трогаем),
   вызвать `Syncer::run`, собрать `LibsOutcome { classpath: Vec<PathBuf>, natives }` одним
   проходом по `cached.chain(downloaded)`.
8. Удалить `libs/download.rs` и `libs/register.rs` целиком — их содержимое разошлось по
   `sync.rs` и `libs/storage.rs`.

**Проверка:** тест на `Syncer::run` с фиктивным `SyncStore` — при полностью заполненном кэше
`downloaded` пуст, `cached.len() == wanted.len()`, и `op` получил `current == total`.

### Этап 3 — перевод assets

9. `assets/storage.rs` — `impl SyncStore<AssetInfo>` по образцу шага 6.
10. `assets/mod.rs::sync` — сигнатура §2.3, возвращает `Result`. Логика разбора `objects`
    остаётся в `parse.rs`, но становится чистой функцией «манифест → `Vec<AssetInfo>`» без
    загрузки и БД.
11. Удалить `assets/download.rs` (там только дублирующий `process_futures`) и
    `assets/register.rs`.

**Проверка:** `cargo check`; дублирующихся констант и `process_futures` в дереве больше нет
(`grep -rn "CONCURRENT_TASKS_COUNT\|process_futures" src/` даёт по одному вхождению).

### Этап 4 — `init.rs`

12. Переписать `install` по сигнатуре §2.3. Что уходит:
    - фолбэк `manifest["id"]` → `manifest["version"]` (:50-64) — версия уже есть в
      `rec.version`, читать её заново из файла не нужно;
    - хардкод `ManifestType::Prism` (:70) — провайдер берётся из `rec.meta_provider`;
    - `paths.set_version_manifest_file(path)` (:37) — путь возвращается из `download_manifest`
      и используется локально.
13. Расставить стадии через `op`: `FetchManifest` → `DownloadLibs` → `DownloadAssets`, каждая
    `start_stage` / `complete_stage`. `Syncer` внутри стадии сам шлёт `update_stage`.
14. `service.rs:96` — принять результат: `let launch_info = instance::init::install(...).await?;`
    и завершить операцию `op.finish(Outcome::Completed)` / `Failed` по итогу.

**Проверка:** `cargo check` по `instance/` чист; сквозной прогон установки инстанса (см. §6).

### Этап 5 — устойчивость (можно отдельным PR)

15. Миграция `0004`: `UNIQUE(hash)` на `libraries` и `assets` + `INSERT ... ON CONFLICT DO NOTHING`
    в обоих `SyncStore::register`. Без этого два параллельных инстанса, качающих одну библиотеку,
    порождают дубликаты.
16. Убрать `.unwrap()` из `download_manifest` (:27-34) и `get_assets_manifest` (:55); путь
    сохранения передавать параметром, а не выводить нарезкой URL.
17. `META_BASE_URL` в `parse/prism.rs:15` → из `MetaProvider`.

---

## 4. Что переиспользуем, а не пишем заново

| Уже есть | Где | Роль в рефакторинге |
|---|---|---|
| `Downloadable` | `utils/download/mod.rs:39` | Границa `Syncer`; оба типа уже реализуют |
| `Download<T>::download_with_checksum` | `utils/download/mod.rs:54` | Загрузка с редиректами и SHA1 — не трогать |
| `BufferPool` | `utils/download/buffer.rs` | Пул буферов внутри `Syncer` |
| `OperationHandle` | `data/registry/operation/handle.rs` | `start_stage` / `update_stage` / `complete_stage` — готов |
| `ProcessTarget` | `data/registry/operation/message/target.rs` | Заполнение `target` в `update_stage` |
| `LauncherPaths` | `data/layout.rs` | `libraries()`, `assets()`, `meta()`, `instances()` |
| `VersionCatalog` / `VersionSummary` | `version/` | Источник `url` + `provider` для установки |
| `maven::build_file_path` | `utils/maven.rs` | `save_path` для libs |

---

## 5. Решения, которые надо принять до кода

**Политика провала загрузки.** Для libs непустой `failed` должен быть фатальным (неполный
classpath = неработающий инстанс). Для assets — вероятно, нет. Зафиксируйте явно, иначе
поведение унаследуется случайно.

**`meta_provider: String` в `InstanceRecord`.** Сейчас строка, а нужен `MetaProvider` из
`version::provider`. Это стык с модулем версий: `install` обязан знать провайдера, чтобы выбрать
парсер. Либо `TryFrom<&str>` на границе БД, либо смена типа поля с миграцией — но строку
в `match` тащить не стоит.

**Компиляционная проверка запросов.** `WHERE hash IN (...)` с переменным числом параметров
не выражается макросом `sqlx::query!`. Варианты: `QueryBuilder` с рантайм-биндингом (теряется
compile-time проверка) либо временная таблица + JOIN (проверка сохраняется, сложнее). Плюс
чанковать по ~500 параметров — у SQLite есть предел числа переменных в запросе. Рекомендую
`QueryBuilder`: потеря проверки на двух запросах приемлема, а полный скан таблицы ассетов на
каждый инстанс — нет.

**Куда девать `CONCURRENT_TASKS_COUNT`.** Сейчас захардкожено в двух местах, после рефакторинга —
в одном. Логичное место в перспективе — `Config` (§4.4 `ANALYSIS.md`, общий лимит на процесс,
чтобы libs и assets вместе не дали 200 параллельных загрузок). В рамках этого рефакторинга
достаточно константы в `sync.rs`; выносить в конфиг — отдельная задача.

---

## 6. Верификация

**Компиляция.** После каждого этапа `cargo check`; ошибки в `src/instance/**` должны убывать.
Учтите, что крейт не собирается и по независимым причинам (`main.rs:33` объявляет несуществующий
`mod operation;`), поэтому смотрите на диагностики именно по `src/instance/`.

**Отсутствие дублей.** `grep -rn "CONCURRENT_TASKS_COUNT\|CONCURRENT_BUFFERS_SIZE\|process_futures" src/`
— по одному вхождению каждого. `grep -rn "crate::websocket" src/` — пусто.

**Юнит-тесты `Syncer`.** С фиктивным `SyncStore` (в памяти, без сети и БД):
полный кэш → `downloaded` пуст и `current == total` с первого события прогресса;
пустой кэш → `wanted.len()` загрузок; провал одного элемента → он в `failed`, остальные
в `downloaded`.

**Classpath.** Тест на сборку `Vec<PathBuf>` → `env::join_paths` → строка содержит `N-1`
разделителей для `N` путей. Сейчас этот инвариант нарушен, и без теста регрессия вернётся.

**Сквозной прогон.** Создать инстанс через `InstanceService::create` + `install`, убедиться, что:
`~/.sonata/libraries` и `~/.sonata/assets/objects` заполняются; повторный `install` того же
инстанса не качает ничего заново и сразу рапортует 100%; в БД нет дублей по `hash`
(`SELECT hash, COUNT(*) FROM libraries GROUP BY hash HAVING COUNT(*) > 1` — пусто).
