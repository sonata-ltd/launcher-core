# Целевая структура ядра

Единый ориентир для рефакторинга под сплит **library / web-server** (см. `ARCHITECTURE_AUDIT.md`).
Принципы, на которых стоит эта раскладка:

1. **Два cargo-feature в одной кодовой базе.** Библиотека (нативные rust-фронтенды, прямые вызовы) компилируется без tide; `server` добавляет tide-стек и HTTP/WS-транспорт.
2. **Вертикальные срезы** по образцу модуля `java` (`model` / `error` / `service` / `storage` / `registry`).
3. **Транспортно-независимый домен.** Домен производит типизированные события в шину; сериализация и отправка живут только в `server/`.
4. **Тип-нагрузка живёт со своим владельцем.** `CoreEvent` — тонкий агрегатор-индекс, payload-типы принадлежат своим срезам.
5. **YAGNI по размещению.** `operation` — инфраструктурный реестр задач, наследник `data/registry/task/`, поэтому лежит в `data/registry/`, а не в отдельной папке в корне.

---

## Cargo.toml (фичи) и точки входа

```toml
[features]
default = []
server  = ["dep:tide", "dep:tide-websockets", "dep:http-types"]

[[bin]]
name = "launcher"
required-features = ["server"]
```

**`src/lib.rs`** — корень библиотеки, объявляет модули и реэкспортирует публичный API:
```rust
pub mod bus;
pub mod data;
pub mod instance;
pub mod java;
pub mod utils;

#[cfg(feature = "server")]
pub mod server;
```

**`src/main.rs`** — тонкий bin, доступен только с `--features server`: парсит аргументы/конфиг, поднимает `GlobalState`, вызывает `server::run(state)`. Никакой доменной логики.

---

## Полное дерево

```
src/
├── lib.rs                       # корень библиотеки
├── main.rs                      # тонкий bin (required-features = ["server"])
│
├── bus/                         # шина событий (библиотека)
│   ├── mod.rs                   #   EventBus
│   └── event.rs                 #   CoreEvent + From<_>
│
├── data/                        # инфраструктура и состояние (библиотека)
│   ├── mod.rs                   #   GlobalState
│   ├── config.rs                #   Config
│   ├── layout.rs                #   раскладка каталогов (бывш. definition.rs + root.rs)
│   ├── db/
│   │   └── mod.rs               #   Database, Result/Error БД
│   └── registry/
│       ├── mod.rs               #   реэкспорты реестров
│       ├── instance/
│       │   ├── mod.rs           #   InstanceRegistry
│       │   └── operations.rs    #   broadcast-обвязка инстансов
│       └── operation/           # ← бывш. task/, подсистема операций
│           ├── mod.rs           #   OperationRegistry
│           ├── handle.rs        #   OperationHandle
│           ├── snapshot.rs      #   OperationSnapshot, Progress
│           ├── message.rs       #   OperationMessage / OperationEvent / стадии / ProcessTarget
│           └── status.rs        #   Lifecycle, Outcome, ProgressUnit, OperationId
│
├── java/                        # ЭТАЛОН вертикального среза (без изменений, минус parse/)
│   ├── mod.rs
│   ├── model.rs                 #   JavaRuntime, NewJavaRuntime
│   ├── error.rs                 #   JavaError, JavaResult
│   ├── service.rs               #   JavaService
│   ├── storage.rs               #   JavaStore + DbJavaStore
│   ├── registry.rs              #   JavaRegistry
│   └── utils.rs                 #   хелперы java
│
├── instance/                    # вертикальный срез instance
│   ├── mod.rs                   #   фасад среза
│   ├── model.rs                 #   Instance, InstanceRecord, InitData, RunData
│   ├── error.rs                 #   InstanceError, InstanceResult
│   ├── service.rs               #   InstanceService (init/run/change_field/get_page/scan)
│   ├── storage.rs               #   InstanceStore + DbInstanceStore
│   ├── scan.rs                  #   ScanMessage, ScanData, ScanIntegrity, ScanInfo
│   ├── paths.rs                 #   InstancePaths (бывш. paths/mod.rs, сплющено)
│   ├── download/
│   │   ├── mod.rs               #   оркестрация загрузки
│   │   ├── manifest.rs          #   выбор/загрузка манифеста
│   │   ├── sync.rs              #   общий generic-синхронизатор (см. §дедуп)
│   │   ├── libs/
│   │   │   ├── mod.rs
│   │   │   ├── parse/{mod,prism}.rs
│   │   │   └── register.rs
│   │   └── assets/
│   │       ├── mod.rs
│   │       ├── parse.rs
│   │       └── register.rs
│   ├── launch/
│   │   ├── mod.rs               #   LaunchInfo, LaunchInfoBuilder
│   │   ├── args.rs              #   ArgType
│   │   ├── natives.rs           #   распаковка нативных либ
│   │   ├── execute.rs           #   запуск процесса
│   │   └── traits.rs
│   └── options/
│       ├── mod.rs               #   Options, ReadPage, OptionUpdate, InstanceFields
│       ├── pages/
│       │   ├── mod.rs           #   реестр страниц
│       │   ├── overview.rs      #   OverviewFields
│       │   └── settings.rs      #   SettingsFields
│       └── tests.rs
│
├── utils/                       # общие утилиты (библиотека)
│   ├── mod.rs
│   ├── download/
│   │   └── mod.rs               #   BufferPool, Download<T: Downloadable>, Downloadable
│   ├── maven.rs                 #   maven-координаты
│   └── unify.rs / manifest URL  #   единый источник URL/провайдеров манифестов
│
└── server/                      # ТОЛЬКО feature="server", весь #[cfg(feature="server")]
    ├── mod.rs                   #   сборка tide::Server, регистрация роутов, run(state)
    ├── relay.rs                 #   ЕДИНСТВЕННАЯ точка ws.send_json
    ├── envelope.rs              #   BaseMessage + wrap(CoreEvent) -> WireMessage
    ├── request.rs               #   распаковка tide::Request (бывш. EndpointRequest из main.rs)
    ├── instance.rs              #   роуты инстансов (бывш. endpoint/instance.rs)
    ├── java.rs                  #   роуты java (бывш. endpoint/java.rs)
    ├── versions.rs              #   роуты версий (бывш. endpoint/versions.rs)
    └── debug.rs                 #   отладочные роуты (бывш. endpoint/debug.rs + debug_tasks из main.rs)
```

---

## Что в каждом файле

### `bus/` — шина событий

**`bus/mod.rs`**
```rust
#[derive(Clone)]
pub struct EventBus { tx: Sender<CoreEvent> }   // async_broadcast, set_overflow(true)

impl EventBus {
    pub fn new(cap: usize) -> Self;
    pub fn publish(&self, event: impl Into<CoreEvent>);   // try_broadcast, не блокирует и не паникует
    pub fn subscribe(&self) -> Receiver<CoreEvent>;
}
```
Типизированный `CoreEvent` вместо нынешнего `Sender<serde_json::Value>` — нативный фронтенд подписывается и матчит enum напрямую, без сериализации.

**`bus/event.rs`** — тонкий агрегатор-индекс, ссылается на владельцев payload:
```rust
#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum CoreEvent {
    Operation(data::registry::operation::OperationMessage),
    Scan(instance::scan::ScanMessage),
    Option(instance::options::OptionUpdate),
}

impl From<OperationMessage> for CoreEvent { /* Operation */ }
impl From<ScanMessage>      for CoreEvent { /* Scan */ }
impl From<OptionUpdate>     for CoreEvent { /* Option */ }
```
Envelope (`BaseMessage`) здесь **нет** — он навешивается транспортом в `server/envelope.rs`.

### `data/` — инфраструктура и состояние

**`data/mod.rs`**
```rust
pub struct GlobalState {          // бывш. GlobalDataState<'a>, БЕЗ лайфтайма
    pub db: Arc<Database>,
    pub bus: EventBus,
    pub operations: Arc<OperationRegistry>,
    pub instances: Arc<InstanceService>,
    pub java: Arc<JavaService>,
    pub config: Config,
}
```
Лайфтайм ушёл вместе с `Task<'a>`. `GlobalState` — публичный тип библиотеки, native-фронтенд создаёт его напрямую.

**`data/config.rs`** — `Config` (пути, настройки ядра).
**`data/layout.rs`** — единое знание о раскладке каталогов (корень, `libraries`, `instances`, `natives`), собранное из `definition.rs` и `root.rs`; типизированные ошибки вместо строк и `format!("...successfully")`.
**`data/db/mod.rs`** — `Database { pool }`, тип `Result`/`Error` доступа к БД.

**`data/registry/instance/`**
- `mod.rs` — `InstanceRegistry` (owned, без `Task<'a>`); `InstanceRecord::new_shared` собран здесь, не разорван.
- `operations.rs` — broadcast-обвязка; **чинит копипаст-баг** подписки на `tasks.notifier` (должен быть `instances.notifier`); публикует в `EventBus`, а не в `serde_json::Value`.

**`data/registry/operation/`** — подсистема операций, наследник `task/`:
```rust
// status.rs
#[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize, TS)]
pub struct OperationId(u32);                       // newtype, wire-фиксированная ширина
pub enum Lifecycle { Started, InProgress, Completed, Failed }   // ← ProcessStatus (фаза)
pub enum Outcome   { Completed, Failed, Cancelled }             // ← StageStatus + OperationStatus (итог)
pub enum ProgressUnit { Bytes, Items, Percent }

// message.rs
pub struct OperationMessage { pub operation_id: OperationId, pub data: OperationEvent }
pub enum OperationEvent { Start(OperationStart), Update(OperationUpdate), Finish(OperationFinish) }
pub struct OperationStart  { pub stages: Vec<OperationStage> }
pub struct OperationFinish { pub outcome: Outcome }
pub enum OperationUpdate {
    Determinable   { stage: OperationStage, status: Lifecycle, target: Option<ProcessTarget>, current: usize, total: usize },
    Indeterminable { stage: OperationStage, status: Lifecycle },
    Completed(StageResult),
}
pub enum OperationStage { FetchManifest, DownloadLibs, DownloadAssets, VerifyFiles, CreateStructure, ScanInstances }
pub struct StageResult { pub status: Outcome, pub stage: OperationStage, pub duration_secs: f64, pub error: Option<StageError> }
pub struct StageError  { /* code + message */ }
pub enum ProcessTarget { File { .. }, Dir { .. } }   // generic; ::Instance удалён (дублировал ScanData)
pub enum TargetStatus  { File(FileStatus), Dir(DirStatus) }
pub enum FileStatus    { Downloading, Downloaded, FailedToDownload }
pub enum DirStatus     { Created, FailedToCreate }

// snapshot.rs
pub struct OperationSnapshot {                     // бывш. Task<'a>, owned; хранимое состояние + wire (debug/status)
    pub id: OperationId, pub name: String,
    pub stage: Option<OperationStage>, pub status: Lifecycle,
    pub progress: Progress, pub message: Option<String>,
}
pub enum Progress { Determinable { current: usize, total: usize }, Indeterminable }

// handle.rs — создаётся доменом (InstanceService), знает стадии; НЕ держит ws/req
pub struct OperationHandle { id: OperationId, bus: EventBus, snapshots: Arc<Mutex<HashMap<OperationId, OperationSnapshot>>> }
impl OperationHandle {
    fn emit(&self, data: impl Into<OperationEvent>);   // fold в snapshot + bus.publish
    pub fn start_stage(&self, stage: OperationStage);
    pub fn update(&self, stage: OperationStage, current: usize, total: usize, target: Option<ProcessTarget>);
    pub fn complete_stage(&self, result: StageResult);
    pub fn finish(self, outcome: Outcome);
}

// mod.rs — создаётся один раз в GlobalState, как Database
pub struct OperationRegistry { active: Arc<Mutex<HashMap<OperationId, OperationSnapshot>>>, bus: EventBus, next_id: AtomicU32 }
impl OperationRegistry {
    pub fn begin(&self, name: String, stages: Vec<OperationStage>) -> OperationHandle;
    pub fn snapshot_all(&self) -> Vec<OperationSnapshot>;   // для server/debug.rs
}
```
`From<OperationUpdate/Start/Finish> for OperationEvent` — оставить (питают `emit(impl Into<..>)`), лайфтаймы `<'a>` с них снять.

### `java/` — эталон, без изменений

Срез уже правильный: `service.rs` принимает `db`, доступ через `JavaStore`, ошибки — `JavaError`. Единственная правка — удалить осиротевший `java/parse/` (не компилируется, ссылается на несуществующие типы). `DEFAULT_JAVAREG_CACHE_SIZE` использовать в `JavaService::new` вместо хардкода `100`, либо удалить.

### `instance/` — вертикальный срез

**`instance/model.rs`**
```rust
pub struct Instance { .. }
pub struct InstanceRecord { .. }     // бывш. InstanceDataRow; impl собран здесь целиком, не разорван по деревьям
pub struct InitData { .. }           // распакованные параметры init (вместо tide::Request)
pub struct RunData  { .. }
```
**`instance/error.rs`** — `InstanceError` (thiserror) + `InstanceResult`; свой `NotFound(i64)` вместо `GlobalAppDataError::TaskNotFound` для инстансов.
**`instance/storage.rs`** — `trait InstanceStore: Send + Sync` + `DbInstanceStore { db: Arc<Database> }` (как `JavaStore`/`DbJavaStore`); сырые sqlx-запросы уходят из логики за трейт.
**`instance/service.rs`**
```rust
pub struct InstanceService { storage: Arc<dyn InstanceStore>, registry: Arc<InstanceRegistry>, ops: Arc<OperationRegistry> }
impl InstanceService {
    pub async fn init(&self, data: InitData) -> InstanceResult<(Instance, LaunchInfo)>;  // OperationHandle из ops, НЕ ws/req
    pub async fn run(&self, data: RunData) -> InstanceResult<()>;
    pub async fn change_field(&self, change: InstanceFields, id: i64) -> InstanceResult<()>;
    pub async fn get_page(&self, page: PageKind, id: i64) -> InstanceResult<InstanceFields>;
    pub async fn scan(&self, range: (u32, u32)) -> InstanceResult<()>;   // публикует ScanMessage в bus
}
```
Поглощает нынешние `init.rs`, `list.rs` и доменную часть `utils/db/register_instance`. Двойное ведение прогресса (`update_task` + WS) убрано — только `OperationHandle`.

**`instance/scan.rs`** — owned типы результата сканирования:
```rust
pub struct ScanMessage   { pub data: ScanData }   // envelope снят
pub struct ScanData      { pub integrity: ScanIntegrity, pub info: Option<ScanInfo> }
pub struct ScanIntegrity { pub instance_path: Option<String> }
pub struct ScanInfo      { pub id: i64, pub name: String, pub version: String, pub loader: String }
```
**`instance/paths.rs`** — `InstancePaths` (сплющенный `paths/mod.rs`); легаси-геттеры без вызовов (`instance_manifest_file`, `instances_list_file`, `headers`, `metacache_file`, `root`) удалить.

**`instance/download/`** — оркестрация загрузки:
- `mod.rs` — сборка pipeline, вызывает `sync` для libs и assets.
- `manifest.rs` — выбор/загрузка манифеста; мёртвые `is_asset_downloaded`/`is_array_exists` удалить; хардкод URL заменить ссылкой на единый источник (`utils`).
- `sync.rs` — **новый** generic-синхронизатор поверх `Downloadable`: «выбрать кэшированные из БД → найти отсутствующие по hash → скачать пачкой → зарегистрировать». Устраняет тройное дублирование pipeline и `process_futures` между libs/assets.
- `libs/`, `assets/` — специфика источников (`parse`, `register`), общий конвейер уходит в `sync.rs`. Прогресс — через `&OperationHandle`, не через `OperationWsMessageLocked`.

**`instance/launch/`** — сборка команды запуска:
- `mod.rs` — `LaunchInfo`, `LaunchInfoBuilder`; переименовать `java_bin_path: JavaRuntime` (не «path»); опечатку `fill_defauls` → `fill_defaults`.
- `args.rs`, `natives.rs`, `traits.rs` — как есть.
- `execute.rs` — **убрать захардкоженный** `/Users/quartix/.sonata/instances/1.5.2/natives`; путь — из `data::layout`.

**`instance/options/`**
```rust
// mod.rs
pub struct OptionUpdate { pub option: InstanceFields }       // бывш. OptionUpdateMessage, без BaseMessage
pub enum InstanceFields { Overview(OverviewFields), Settings(SettingsFields) }
pub trait ReadPage { .. }
// pages/overview.rs, pages/settings.rs — OverviewFields, SettingsFields
```
Пустые структуры-заглушки (`General`, `Mods`, `Worlds`, `Resourcepacks`, `Shaderpacks`, `Logs`), `Options::new`, `Options::retrieve_all` — удалить, если не используются.

### `utils/` — общие утилиты

- `download/mod.rs` — `BufferPool`, `Download<T: Downloadable>`, трейт `Downloadable` (хорошая абстракция, оставить).
- `maven.rs` — координаты; `coord_to_path` под `#[allow(dead_code)]` — удалить, если не нужен.
- единый источник URL/провайдеров манифестов вместо трёх енумов (`ManifestType`/`ManifestTypes`/`MetaProviders`) и захардкоженных URL в 4 местах.
- `async_trait` брать из прямой зависимости `async-trait`, а не из `tide::utils`/`surf::utils` (скрытая зависимость домена от tide-стека).

### `server/` — транспорт (только feature `server`)

**`server/mod.rs`** — `pub fn run(state: GlobalState)`: строит `tide::Server`, регистрирует роуты, слушает порт.
**`server/relay.rs`** — единственная точка отправки, без `unwrap`/паники:
```rust
pub async fn relay(mut rx: Receiver<CoreEvent>, ws: WebSocketConnection) {
    while let Ok(ev) = rx.recv().await {
        if ws.send_json(&envelope::wrap(ev)).await.is_err() { break }
    }
}
```
**`server/envelope.rs`** — wire-конверт живёт здесь, не в домене:
```rust
pub struct BaseMessage { pub message_id: String, pub timestamp: DateTime<Utc>, pub correlation_id: Option<String>, .. }
pub struct WireMessage { pub base: BaseMessage, #[serde(flatten)] pub event: CoreEvent }
pub fn wrap(event: CoreEvent) -> WireMessage;
```
**`server/request.rs`** — `EndpointRequest` (переезжает из `main.rs`); распаковка `tide::Request` в `InitData`/`RunData`/`InstanceFields`. Домен эти типы не импортирует.
**`server/{instance,java,versions}.rs`** — хендлеры: распаковать запрос → вызвать сервис → на WS подписать `bus.subscribe()` и запустить `relay`. Здесь исчезают оба сломанных `send`, `OperationWsMessage`, `OperationWsExt`, `WsMessageType`.
**`server/debug.rs`** — отладочные роуты; список активных операций через `state.operations.snapshot_all()` вместо `get_all_tasks_json()`.

---

## Направления зависимостей

```
utils/            → leaf
data/db, config, layout → leaf
data/registry/operation → использует bus (публикация); владеет DTO операций
data/registry/instance  → использует bus; ссылается на instance::model
java/             → data (db)
instance/         → data (реестры, db, layout), utils; владеет scan/options
bus/ (CoreEvent)  → data::operation + instance (агрегатор, стрелка вниз)
server/           → всё вышеперечисленное + tide  (#[cfg(feature="server")])
```

Взаимные ссылки `data ↔ bus`, `data ↔ instance` — внутри одного крейта легальны; проблемой стали бы только при выносе `bus` в отдельный крейт (не наш сценарий).

**Сборки:**
- `cargo build` (без фич) — компилируется всё, кроме `server/`; tide в графе нет.
- `cargo build --features server` — добавляет `server/` и tide-стек.

---

## Переименования

| Было | Стало |
|---|---|
| `websocket/messages/`, `WsMessage<'a>` | `bus/event.rs`, `CoreEvent` |
| `instance/websocket/`, `OperationWsMessage`, `OperationWsExt`, `WsMessageType` | `data/registry/operation/handle.rs` + `server/relay.rs` |
| `data/registry/task/`, `Task<'a>` | `data/registry/operation/`, `OperationSnapshot` |
| `ProcessStatus` | `Lifecycle` |
| `StageStatus` + `OperationStatus` | `Outcome` |
| `Sender<serde_json::Value>` | `EventBus` / `Sender<CoreEvent>` |
| `GlobalDataState<'a>` | `GlobalState` |
| `InstanceDataRow` | `InstanceRecord` |
| `endpoint/` | `server/` |
| `EndpointRequest` (в `main.rs`) | `server/request.rs` |
| `BaseMessage` (в домене) | `server/envelope.rs` |
| `data/definition.rs` + `root.rs` | `data/layout.rs` |
| `instance/paths/mod.rs` | `instance/paths.rs` |
| `ProcessTarget::Instance` | удалён (дублировал `ScanData`) |

## Удаляется целиком

`websocket/`, `instance/websocket/`, `data/registry/task/`, `endpoint/`, `java/parse/`, `utils/system.rs`, `utils/metacache/`, `root.rs`, `dev.db` (из git), `collect_code.sh` (→ `scripts/` или удалить).

## Когда `operation` поднимать в корень `src/`

Пока единственный эмиттер прогресса — `instance`, подсистема живёт в `data/registry/operation/` как generic-реестр задач. Если второй эмиттер появится (например, загрузка java-рантайма через операции), `data/registry/operation/` → верхнеуровневый `operation/`: `git mv` + правка путей.
