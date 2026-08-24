# Разделение на библиотеку и подготовка к iced

## Зачем и что мешает сейчас

Ядро — бинарный крейт без `lib.rs`. Всё, что нужно GUI, физически существует и работает
(`create → install → launch` проходит целиком), но снаружи недоступно: `main.rs` объявляет
`mod bus; mod data; mod utils;` приватно, а `pub mod instance / java / version` публичны только
внутри бинарника, которого у GUI не будет.

Работа делится на четыре этапа. Первые два — механические и безопасные. Третий вскрывает
настоящие дыры: GUI спрашивает у ядра вещи, которых оно пока не умеет. Четвёртый — мост к iced,
и там есть две ловушки, о которых лучше знать до, а не после.

---

## Этап 1 — механика

### 1.1. Раскладка файлов

```
src/lib.rs          ← НОВЫЙ: объявляет модули, задаёт публичную поверхность
src/main.rs         ← остаётся бинарником, но только CLI
src/cli/mod.rs      ← НОВЫЙ: то, что сейчас в main.rs ниже `run()`
src/cli/progress.rs ← ПЕРЕЕЗД из src/progress.rs
```

`src/cli/*` намеренно лежит внутри `src/`: пока `lib.rs` не объявляет `mod cli;`, эти файлы
компилируются только в бинарник. Имя каталога делает это очевидным без комментария.

`Cargo.toml` править не нужно — cargo сам подхватывает `src/lib.rs` и `src/main.rs` в одном
пакете. Отдельные секции `[lib]`/`[[bin]]` понадобятся только если захотите переименовать
бинарник (сейчас он получит имя пакета, `sonata_launcher_core`).

### 1.2. `src/lib.rs`

```rust
pub mod bus;
pub mod data;
pub mod instance;
pub mod java;
pub mod utils;
pub mod version;

pub use data::{AppError, Config, GlobalState};

/// Ядро уезжает в состояние iced-приложения через `Arc`, а iced требует
/// `Send + Sync` от всего, что переживает между кадрами. Проверка стоит
/// ноль во время выполнения и ломает сборку в тот же день, когда кто-то
/// внесёт в дерево `Rc` или `RefCell`, а не через месяц у пользователя.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GlobalState>();
};
```

### 1.3. Что переезжает из `main.rs`

Три строки объявлений (`mod bus; mod data; mod utils;`) удаляются — они теперь в `lib.rs`
как `pub mod`. Всё остальное в `main.rs` начинает обращаться к ядру по внешнему имени:

```rust
use sonata_launcher_core::{
    data::{registry::operation::message::OperationMessage, Config, GlobalState},
    instance::{model::InstanceId, settings::SettingsPatch},
    java::model::NewJavaRuntime,
};
```

### 1.4. `mod config` становится публичным

`src/data/mod.rs:24` — `mod config;` → `pub mod config;`. Без этого встраивающий не может
переопределить корень лаунчера, и GUI обязан хранить данные там же, где CLI.

**Проверка этапа:** `cargo build` собирает и `--lib`, и `--bin`; `cargo test` проходит как
раньше.

---

## Этап 2 — публичная поверхность

### 2.1. `GlobalState::new` должен принимать конфиг

Сейчас (`src/data/mod.rs:57`):

```rust
pub async fn new() -> Result<Self, AppError> {
    let config = Config::init();
    ...
}
```

`Config::init()` возвращает жёстко зашитые дефолты и игнорирует всё остальное, а поля
`root_override` и `DirOverrides` существуют и никем не заполняются. Для бинарника это
незаметно, для библиотеки — дефект: встраивающий не может сказать, где ему держать данные.

```rust
pub async fn new(config: Config) -> Result<Self, AppError>
```

`Config::init()` остаётся как `Default`-подобный конструктор, вызывает его теперь вызывающий.

### 2.2. Что оставить приватным

`InstanceService::get_registry` и `JavaService::get_registry` — `pub(crate)`, и такими и
остаются. После разделения `pub(crate)` означает «внутри библиотеки», то есть GUI до реестров
не дотянется, а это и есть замысел: реестры — единственная дверь к хранилищам, и раздавать
вторую дверь наружу значит вернуть ровно ту проблему, ради которой их сводили в одну.

Хранилища (`DbInstanceStore`, `DbLibStore`, `DbAssetStore`, `DbJavaStore`,
`DbInstanceSettingsStore`) наружу не нужны вообще. Трейты (`InstanceStore`, `JavaStore`,
`SyncStore`) — публичны, они точка расширения.

### 2.3. Именование геттеров

`get_java_srv` / `get_versions_srv` / `get_instances_srv` / `get_options_srv` — приставка `get_`
не в стиле Rust, и в коде GUI это будет видно на каждой строке. Момент разделения —
единственный дешёвый момент переименовать в `java()` / `versions()` / `instances()` /
`options()`. Дальше это уже ломающее изменение для чужого кода.

---

## Этап 3 — дыры, которые обнажит GUI

Это не косметика. Каждый пункт — экран, который нечем нарисовать, или кнопка, которую нечем
нажать.

| Что нужно GUI | Состояние | Что сделать |
|---|---|---|
| Список инстансов | `InstanceStore::list_all` **есть, но не вызывается ниоткуда**; `InstanceRegistry::all()` отдаёт только кэш, то есть после старта — пустоту | `InstanceRegistry::list()` через стор с прогревом кэша, `InstanceService::list()` |
| Список java-рантаймов | нет вовсе: у `JavaStore` только `insert` / `get_by_id` / `exists_by_path` | `JavaStore::list_all` → `JavaRegistry::list` → `JavaService::list` |
| Правка глобальных настроек | `settings_global` заполняется миграцией, наследование работает, менять нечем | `InstanceSettingsStore::patch_global` + `SettingsService::patch_global` |
| Проверка `memory_min ≤ memory_max` | нет | в `SettingsService::patch` и `patch_global` |
| Удаление инстанса | `InstanceRegistry::delete` удаляет строку, каталог на диске остаётся | `InstanceService::delete`, сносящий и каталог |
| Отмена установки | нет | токен отмены в `OperationHandle`, проверка в цикле `Syncer::run` |
| Остановка запущенной игры | `launch` ждёт выхода процесса, `Child` теряется внутри `launch_instance` | вернуть управляемый handle либо держать процессы в реестре |
| Восстановление UI после перезапуска | `OperationRegistry::snapshot_all()` **уже есть** | — |

Первые два пункта блокируют самый первый экран, который вы напишете, — список инстансов и
выпадающий список java. Их стоит сделать до того, как открывать iced.

Отмену и остановку можно отложить: без них GUI работает, просто кнопка «Отмена» не появится.

---

## Этап 4 — мост к iced

### 4.1. Executor: не включайте `tokio`

У iced три фичи исполнителя — `thread-pool` (по умолчанию), `tokio`, `smol`. Фичи `async-std`
нет.

Это не проблема. Ядро не использует ни одного API, которому нужен *конкретный* ambient-рантайм:
`async_std::task::spawn`, `task::sleep`, `async_std::fs` и `async_std::process` работают от
собственного реактора async-std независимо от того, кто опрашивает внешнее будущее; sqlx собран
с `runtime-async-std`; surf через isahc/curl вообще держит свой поток. Любой из трёх
исполнителей iced будет драйвить ядро корректно.

Рекомендация: оставить дефолтный `thread-pool` или взять `smol` — async-std построен на тех же
кирпичах (`async-io`, `async-executor`, `blocking`), так что потоки переиспользуются.
`tokio` включать не надо: он поднимет второй полноценный рантайм со своим пулом потоков ради
нулевой выгоды.

Отдельно, на будущее: async-std объявлен discontinued, авторы рекомендуют переезд на smol.
Делать это сейчас не нужно и в объём разделения не входит, но зафиксировать как задачу стоит —
и разделение на библиотеку как раз тот момент, когда такой переезд перестанет затрагивать GUI.

### 4.2. Подписка на шину: `run_with`, а не `run`

Ловушка в сигнатуре:

```rust
pub fn run<S>(builder: fn() -> S) -> Subscription<T>
```

`fn() -> S` — **указатель на функцию, а не замыкание**. Захватить `EventBus` в него нельзя.
Нужен второй конструктор:

```rust
pub fn run_with<D, S>(data: D, builder: fn(&D) -> S) -> Subscription<T>
where D: Hash + 'static
```

`EventBus` не реализует `Hash`, поэтому его надо завернуть. И `Hash` здесь — не хеш содержимого,
а **идентичность подписки**: по нему iced решает, продолжать существующий поток или поднять
новый. Хешировать надо константу, иначе каждый `view()` будет пересоздавать подписку и терять
события.

```rust
#[derive(Clone)]
struct CoreEvents(EventBus);

impl std::hash::Hash for CoreEvents {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Идентичность, а не содержимое: шина одна на приложение.
        "sonata::core::events".hash(state);
    }
}

fn subscription(&self) -> Subscription<Message> {
    Subscription::run_with(CoreEvents(self.core.bus.clone()), |events| {
        // `EventBus::subscribe` отдаёт async_broadcast::Receiver,
        // а он уже Stream<Item = CoreEvent> + Send.
        events.0.subscribe().map(Message::Core)
    })
}
```

Сверьте сигнатуры со своей версией iced: `run_with` появился не сразу, в старых выпусках
на его месте `run_with_id`.

**Про потерю событий.** Шина создана с `set_overflow(true)` (`src/bus/mod.rs:19`), и `Stream`
у `async_broadcast` молча пропускает вытесненные элементы. Для прогресса это правильно: каждое
событие несёт полное состояние, и потеря промежуточного ничего не значит. Для `ScanMessage` и
`OptionUpdateMessage` — значит: пропущенное событие оставит экран устаревшим навсегда. Поэтому
GUI должен уметь перезапросить состояние (см. дыры из этапа 3), а не считать шину источником
истины.

### 4.3. `Message` обязан быть `Clone`, а доменные ошибки — нет

`button::on_press(msg)` и большинство виджетов требуют `Message: Clone`. При этом
`InstanceError` не `Clone` и стать им не может: внутри `io::Error`, `Box<dyn Error>` и цепочки
`#[source]`.

Оборачивать в `String` — терять `source()` и всю диагностику, ради которой ошибки и типизировали.
Правильная граница — `Arc`:

```rust
#[derive(Debug, Clone)]
enum Message {
    Core(CoreEvent),
    Instances(Result<Vec<InstanceRecord>, Arc<InstanceError>>),
    Installed(Result<(), Arc<InstanceError>>),
}

Task::perform(
    async move { instances.install(id).await.map_err(Arc::new) },
    Message::Installed,
)
```

`Arc<E>` — `Clone`, `Display` и `Error` доходят через `Deref`, цепочка причин цела.

### 4.4. Мина в `Database::init`

`src/data/db/mod.rs:41-62`: в отладочной сборке инициализация вызывает
`dotenvy::dotenv_override()` и, если найдёт `DATABASE_URL`, использует его **вместо** пути из
`LauncherPaths`.

Для CLI, запускаемого из корня репозитория, это удобно. Для GUI это отложенная мина: собранный
в debug бинарник, запущенный из другого каталога, либо подхватит чужой `.env`, либо напишет
warning и молча уйдёт на дефолтный путь — а вы будете смотреть на пустой список инстансов и
искать ошибку в SQL.

Чинится вместе с 2.1: `Config` получает `database_url: Option<String>`, `Database::init`
перестаёт читать окружение вовсе, а чтение `.env` переезжает в `src/cli/` — туда, где оно и
уместно.

### 4.5. Состояние приложения

```rust
struct Launcher {
    core: Arc<GlobalState>,
    instances: Vec<InstanceRecord>,
    // ...
}
```

Все сервисы уже `Arc<...>` и `Send + Sync`; проверка из 1.2 это закрепляет. В `Task::perform`
уезжает клон нужного сервиса (`self.core.instances()`), а не ссылка на `self`.

Отдельно про `launch`: он ждёт завершения игры. В iced это нормально — `Task` не блокирует UI, —
но задача будет висеть часами, и отменить её нечем (см. этап 3).

---

## Порядок работ

1. **Этап 1 целиком.** Один коммит, чисто механический. Проверка: `cargo build`, `cargo test`,
   CLI по-прежнему проходит `create → install → launch`.
2. **4.4 + 2.1.** `Config` c `database_url`, `Database::init` без окружения, dotenv в `cli/`.
   Делать до GUI, а не после.
3. **Первые два пункта этапа 3** — списки инстансов и java-рантаймов. Без них первый же экран
   рисовать нечем.
4. **Скелет iced**: окно, список инстансов, подписка на шину по 4.2, установка через `Task` по
   4.3. На этом шаге ядро больше не трогается — и это главная проверка, что разделение удалось.
5. Остальное из этапа 3 по мере появления экранов.

Пункты 2 и 3 можно делать параллельно с 4 разными руками: они не пересекаются по файлам.

## Что считать успехом

`src/cli/` и будущий GUI используют только `pub`-поверхность библиотеки, ни один из них не
знает слов `sqlx`, `surf` и `async_broadcast`, и добавление второго потребителя не потребовало
ни одной правки в `src/instance/`, `src/java/` и `src/version/`.
