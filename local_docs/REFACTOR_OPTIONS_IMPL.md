# Рефакторинг `instance/options` — реализация

## Context

`instance/options` — последний срез, оставшийся в старом поколении архитектуры вместе с
`download/*`. Отличие от `download/*` в том, что там ломалась механика, а здесь сломан контракт:
домен принимает `tide::Request`, что делает библиотечную сборку ядра невозможной в принципе.

Главная находка при разборе — **срез не просто не компилируется, у него мёртв весь путь чтения**.
`Overview::insert` и `Settings::upset` не вызываются ниоткуда, `DbInstanceStore::insert` пишет
только в `instances`. Строки в `instances_overview` и `instances_settings` не создаются никогда,
а `Overview::from_db` использует `fetch_one`, то есть чтение options возвращает `RowNotFound`
для любого реально существующего инстанса.

### Принятые решения

| Вопрос | Решение |
|---|---|
| Контракт с фронтендом | Свободен, ts-биндинги можно менять |
| Нереализованные страницы | `Mods`, `Worlds`, `Resourcepacks`, `Shaderpacks`, `Logs` удаляются вместе с `Page`-вариантами |
| `name` и `dir` | Единственный источник правды — `instances`; options читает JOIN'ом, `dir` только на чтение |
| Границы | Домен + сервис; HTTP остаётся выключенным, но JSON-диспетчеризация выносится из домена |
| Форма среза | `model` / `error` / `storage` / `service` + `dto` для JSON |
| Писатель в `instances` | Только `InstanceRegistry` — он же обновляет свой кэш |

### Почему `Page` уезжает из домена

Ядро собирается и как rust-библиотека, и как http-сервер. Сейчас, чтобы прочитать playtime,
библиотечный потребитель обязан вызвать `retrieve(db, id, Page::Overview)`, получить `PageResult`,
сматчить его и достать поле. `Page`, `PageResult` и `ChangableOptions` — это форма UI и форма
провода; Rust-вызывающему нужен `options.overview(id).await?.playtime`.

Поэтому домен отдаёт четыре типизированных метода, а тегированные union'ы и разбор
`{id, page, options}` живут в `dto.rs` рядом с `serde_json`.

### Почему имя пишется через registry, а не через `OptionsStore`

`InstanceRegistry` кэширует `Arc<InstanceRecord>`, в котором лежит `name`. Если options правит
`instances.name` своим запросом, кэш протухает и список инстансов показывает старое имя до
перезапуска. Вариант «правим сами, потом инвалидируем кэш» рабочий, но инвалидацию забывают —
она не выражена в типах и не проверяется компилятором.

В проекте уже есть нужный паттерн: `InstanceService::create` не трогает store напрямую, он зовёт
`registry.create`, а тот пишет в store и обновляет кэш. Тем же путём идёт и переименование:
`InstanceRegistry::rename` вызывает уже существующий (и сейчас никем не используемый)
`InstanceStore::update_name` и сам чинит свой кэш.

**Цена решения.** Патч overview перестаёт быть атомарным между `instances` и
`instances_overview` — это две записи, а не одна транзакция. Порядок выбран так, чтобы отказ
был наименее неприятным: сначала пишутся поля overview, переименование последним. Тогда
`NameTaken` — единственная реалистичная ошибка этого запроса — оставляет имя нетронутым.
Если когда-нибудь понадобится настоящая атомарность, она достигается переносом обеих записей в
`InstanceRegistry` с транзакцией внутри, но это заставит срез `instance` знать про таблицы
options.

---

## Дерево до / после

```
instance/options/                     instance/options/
├── mod.rs                            ├── mod.rs        (re-export)
├── model.rs                          ├── error.rs      ← НОВЫЙ
├── tests.rs                          ├── model.rs      (переписан)
└── pages/            ← УДАЛИТЬ       ├── storage.rs    ← НОВЫЙ
    ├── mod.rs        ← УДАЛИТЬ       ├── service.rs    ← НОВЫЙ
    ├── overview.rs   ← УДАЛИТЬ       ├── dto.rs        ← НОВЫЙ
    └── settings.rs   ← УДАЛИТЬ       └── tests.rs      (переписан)

src/websocket/messages/option/        ← УДАЛИТЬ (дубль model.rs в отключённом модуле)
```

Правки вне среза, без которых он не собирается и не работает:

| Файл | Что |
|---|---|
| `instance/model.rs` | `InstanceId`: `sqlx::Type`, `From<i64>`, `Serialize`, `TS` |
| `instance/registry.rs` | дедлок в `get`, новый `rename` |
| `instance/storage.rs` | `insert` в транзакции, создаёт три строки |
| `instance/error.rs` | убрать `OptionNotAvailable`, `OptionsPageWrong` |
| `java/model.rs` | `TS` на `JavaRuntime` |
| `migrations/0003_instances.sql` | `tags TEXT NOT NULL DEFAULT ''` |
| `data/mod.rs` | `OptionsService` в `LauncherServices` |

---

## Этап 1 — `InstanceId`

Сейчас `struct InstanceId(i64)` с приватным полем и без единого sqlx-трейта. Из-за этого падает
`instance/storage.rs` (8 ошибок E0277), и `options/storage.rs` упрётся в то же самое.

```rust
// instance/model.rs
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

#[derive(
    Debug, Clone, Copy, Hash, PartialEq, Eq, Error,
    Serialize, Deserialize, TS, sqlx::Type,
)]
#[error("{0}")]
#[serde(transparent)]
#[sqlx(transparent)]
#[ts(export_to = "./instance/")]
pub struct InstanceId(i64);

impl InstanceId {
    pub fn get(self) -> i64 {
        self.0
    }
}

impl From<i64> for InstanceId {
    fn from(raw: i64) -> Self {
        Self(raw)
    }
}
```

`#[sqlx(transparent)]` на newtype генерирует `Type`, `Encode` и `Decode` разом — руками писать
три impl'а не нужно. `From<i64>` нужен отдельно: `query_as!` применяет `.into()` к колонке, когда
тип поля отличается от типа колонки, и именно этого не хватает в текущих ошибках.

`Serialize` и `TS` нужны потому, что `InstanceId` уходит наружу в `ScanInfo` и в
`OptionUpdateMessage`. Если `ts-rs` не выведет для newtype нужный тип сам, добавить
`#[ts(type = "number")]` — в TypeScript это должно быть `number`, а не объект-обёртка.

**Проверка:** восемь E0277 в `instance/storage.rs` исчезают.

---

## Этап 2 — `InstanceRegistry`

### 2.1. Дедлок в `get`

```rust
// БЫЛО (registry.rs:46-62): cache.lock() на 47-й строке живёт до конца функции,
// а upsert на 59-й лочит тот же Mutex повторно. std::sync::Mutex не реентрантный —
// это дедлок на любом промахе кэша. Плюс guard живёт через .await на 55-й строке.
pub async fn get(&self, id: InstanceId) -> Result<Arc<InstanceRecord>> {
    {
        let cache = self.cache.lock().unwrap();
        if let Some(rec) = cache.get(&id) {
            return Ok(Arc::clone(rec));
        }
    }

    let rec = self
        .store
        .get_by_id(id)
        .await?
        .ok_or(InstanceError::NotFound(id))?;

    // Возвращаем тот же Arc, что положили в кэш. Раньше здесь был
    // Arc::new(rec) отдельно от upsert(rec.clone()) — два разных Arc
    // на одну запись, кэш и вызывающий расходились.
    Ok(self.upsert(rec))
}
```

Явный блок вокруг lock'а лучше, чем `if let` со `.lock()` в скрутинии: во втором случае временный
guard живёт до конца всей конструкции, включая `else`, и любой будущий `.await` внутри снова
даст ту же проблему молча.

### 2.2. `rename`

```rust
pub async fn rename(&self, id: InstanceId, new_name: &str) -> Result<Arc<InstanceRecord>> {
    let current = self.get(id).await?;

    if current.name == new_name {
        return Ok(current);
    }

    self.store
        .update_name(id, new_name)
        .await
        .map_err(|e| match e {
            // instances.name объявлен UNIQUE, а From<sqlx::Error> for DbError
            // уже распознаёт нарушение уникальности (data/db/mod.rs:83)
            DbError::Conflict(_) => InstanceError::NameTaken,
            other => InstanceError::DB(other),
        })?;

    let mut updated = (*current).clone();
    updated.name = new_name.to_string();

    Ok(self.upsert(updated))
}
```

Приватный `remove_from_cache` (`registry.rs:76`) мёртв — `delete` инлайнит его тело. Удалить.

---

## Этап 3 — схема и создание строк-спутников

### 3.1. `migrations/0003_instances.sql`

Одна правка, на месте — новой миграции не нужно, БД пересоздаётся:

```sql
CREATE TABLE IF NOT EXISTS instances_overview(
    instance_id INTEGER PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
    tags TEXT NOT NULL DEFAULT '',
    export_type TEXT NOT NULL DEFAULT 'Sonata',
    playtime INTEGER NOT NULL DEFAULT 0
);
```

`DEFAULT ''` у `tags` нужен, чтобы строку-спутник можно было создать одним
`INSERT INTO instances_overview (instance_id) VALUES (?)`.

```bash
rm -f dev.db && sqlx database create && sqlx migrate run
```

### 3.2. `DbInstanceStore::insert`

```rust
async fn insert(&self, new: &NewInstance) -> Result<InstanceRecord> {
    let mut tx = self.db.pool.begin().await?;

    let rec = sqlx::query_as!(
        InstanceRecord,
        r#"
            INSERT INTO instances (name, dir, version, loader, manifest_url, meta_provider)
            VALUES (?, ?, ?, ?, ?, ?)
            RETURNING
                id AS "id: InstanceId",
                name, dir, version, loader, manifest_url, meta_provider
        "#,
        new.name,
        new.dir,
        new.version,
        new.loader,
        new.manifest_url,
        new.meta_provider
    )
    .fetch_one(&mut *tx)
    .await?;

    // Без этих двух строк чтение options возвращает RowNotFound
    // для каждого созданного инстанса.
    sqlx::query!(
        "INSERT INTO instances_overview (instance_id) VALUES (?)",
        rec.id
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query!(
        "INSERT INTO instances_settings (instance_id) VALUES (?)",
        rec.id
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(rec)
}
```

Транзакция здесь по-настоящему нужна: инстанс без строк-спутников — это инстанс, у которого не
открывается ни одна страница настроек.

Ленивый `INSERT ... ON CONFLICT DO NOTHING` при первом обращении — рабочая альтернатива, но она
оставляет систему в состоянии «строки может не быть», и это состояние обязан учитывать каждый
читающий запрос. Транзакция закрывает вопрос в одном месте.

Аналогичный `AS "id: InstanceId"` нужен в `get_by_id` и `list_all`.

---

## Этап 4 — `options/error.rs` и `options/mod.rs`

```rust
// options/mod.rs — только объявления и re-export.
// Вся логика, которая сейчас лежит здесь (Options, ChangableOptions,
// ChangeRequestBuilder, ChangeRequest), уезжает в dto.rs или удаляется.
pub mod dto;
pub mod error;
pub mod model;
pub mod service;
pub mod storage;

#[cfg(test)]
mod tests;

pub use error::{OptionsError, Result};
pub use model::{Overview, OverviewPatch, PageView, Settings, SettingsPatch};
pub use service::OptionsService;
```

Заодно чистится `instance/mod.rs`: строки 20-22 (`use` трёх типов options), 80 и 100-103
(закомментированные `Options::retrieve` и `Options::change`) — мёртвые комментарии на API,
которого после этого рефакторинга не существует.



```rust
use thiserror::Error;

use crate::{data::db::DbError, instance::model::InstanceId};

#[derive(Debug, Error)]
pub enum OptionsError {
    #[error("instance not found: {0}")]
    InstanceNotFound(InstanceId),

    #[error("instance name is already taken")]
    NameTaken,

    #[error("unknown options page: {0}")]
    UnknownPage(String),

    #[error("java runtime not found: {0}")]
    JavaRuntimeNotFound(i64),

    #[error("malformed patch: {0}")]
    MalformedPatch(#[from] serde_json::Error),

    #[error("instance operation failed: {0}")]
    Internal(String),

    #[error(transparent)]
    Db(#[from] DbError),
}

pub type Result<T> = std::result::Result<T, OptionsError>;
```

Из `InstanceError` уезжают `OptionNotAvailable` и `OptionsPageWrong` — они существуют только ради
options и нигде больше не строятся.

Варианта `UnknownExportType` здесь нет намеренно. Значение в колонке пишем мы сами из закрытого
enum'а, поэтому нераспознанная строка при чтении — это порча данных, и для неё уже есть
`DbError::ResultCorrupted`. Мусор со стороны клиента ловит serde и отдаёт `MalformedPatch`.

---

## Этап 5 — `options/model.rs`

```rust
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use ts_rs::TS;

use crate::{instance::model::InstanceId, java::model::JavaRuntime};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq,
    Serialize, Deserialize, Display, EnumString, TS,
)]
#[ts(export_to = "./options/overview/")]
pub enum ExportType {
    #[default]
    Sonata,
    MultiMC,
    Modrinth,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "./options/overview/")]
pub struct Overview {
    /// Из instances.name — здесь только для чтения страницы.
    /// Запись идёт через InstanceRegistry::rename.
    pub name: String,
    pub tags: String,
    pub export_type: ExportType,
    /// Наигранное время накручивает лаунчер, не клиент —
    /// поэтому поля нет в OverviewPatch.
    pub playtime: i64,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export_to = "./options/overview/")]
pub struct OverviewPatch {
    pub name: Option<String>,
    pub tags: Option<String>,
    pub export_type: Option<ExportType>,
}

impl OverviewPatch {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.tags.is_none() && self.export_type.is_none()
    }

    /// Всё, кроме имени: имя пишет registry, остальное — OptionsStore
    pub fn has_fields(&self) -> bool {
        self.tags.is_some() || self.export_type.is_some()
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "./options/settings/")]
pub struct Settings {
    /// Из instances.dir, только чтение: переименование каталога —
    /// отдельная операция с движением файлов на диске.
    pub dir: String,
    pub java_runtime: Option<JavaRuntime>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export_to = "./options/settings/")]
pub struct SettingsPatch {
    /// Три состояния, а не два:
    ///   None             — поле не трогаем
    ///   Some(None)       — сбросить на рантайм по умолчанию
    ///   Some(Some(id))   — выбрать конкретный
    /// Без двойного Option «сбросить» невыразимо, а serde без
    /// double_option схлопнет уровни в один.
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub java_runtime: Option<Option<i64>>,
}

impl SettingsPatch {
    pub fn is_empty(&self) -> bool {
        self.java_runtime.is_none()
    }
}

/// Тегированный union состояний. Живёт в model, а не в dto,
/// потому что уходит в EventBus — то есть в домен.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "page", rename_all = "lowercase")]
#[ts(export_to = "./options/")]
pub enum PageView {
    Overview(Overview),
    Settings(Settings),
}

/// Payload-only, как ScanMessage и OperationMessage.
/// Конверт BaseMessage (message_id, timestamp, correlation_id)
/// добавляет websocket-слой — домен про него не знает.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "./options/")]
pub struct OptionUpdateMessage {
    pub instance_id: InstanceId,
    pub view: PageView,
}
```

`JavaRuntime` (`java/model.rs`) уже выводит `Serialize` — добавить `TS`, иначе `Settings` не
экспортируется.

Что удаляется вместе со старым `model.rs`:

- рукописный `impl Deserialize for InstanceFields` — в нём баг на строке 76: имя страницы
  сравнивается со строкой `"page"` вместо `"overview"`, то есть любой настоящий payload не
  разбирается. Сообщение теперь только сериализуется, `Deserialize` ему не нужен;
- закомментированный `impl Serialize` на 37 строк;
- `base: BaseMessage` — тип живёт в отключённом `websocket/messages/mod.rs`, отсюда `E0425`.

Вместе с этим удаляется `src/websocket/messages/option/` целиком — это вторая копия того же
`InstanceFields` с тем же багом.

Рукописные `FromStr` и `Display` для `ExportTypes` (`pages/overview.rs:148-167`) заменяются на
`strum`. Текущий `Display` печатает через `{:?}`, а `FromStr` возвращает `DbError` — доменный
тип не должен знать про ошибки БД.

---

## Этап 6 — `options/storage.rs`

```rust
use std::sync::Arc;

use sqlx::prelude::FromRow;
use tide::utils::async_trait;

use crate::{
    data::db::{Database, DbError, Result},
    instance::{
        model::InstanceId,
        options::model::{ExportType, Overview, OverviewPatch, Settings, SettingsPatch},
    },
    java::model::JavaRuntime,
};

#[async_trait]
pub trait OptionsStore: Send + Sync {
    async fn overview(&self, id: InstanceId) -> Result<Overview>;
    async fn settings(&self, id: InstanceId) -> Result<Settings>;

    /// Пишет только instances_overview. instances.name — не его дело,
    /// имя идёт через InstanceRegistry::rename.
    async fn patch_overview(&self, id: InstanceId, patch: &OverviewPatch) -> Result<()>;
    async fn patch_settings(&self, id: InstanceId, patch: &SettingsPatch) -> Result<()>;
}

pub struct DbOptionsStore {
    db: Arc<Database>,
}

impl DbOptionsStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

#[derive(Debug, FromRow)]
struct OverviewRow {
    name: String,
    tags: String,
    export_type: String,
    playtime: i64,
}

#[derive(Debug, FromRow)]
struct SettingsRow {
    dir: String,
    java_id: Option<i64>,
    java_version: Option<String>,
    java_exec_path: Option<String>,
    java_home_path: Option<String>,
    java_vendor: Option<String>,
}

impl SettingsRow {
    fn into_model(self) -> Settings {
        // Все четыре NOT NULL-колонки java_runtimes приходят вместе
        // или не приходят вовсе — LEFT JOIN не даёт частичной строки.
        let java_runtime = match (
            self.java_id,
            self.java_version,
            self.java_exec_path,
            self.java_home_path,
        ) {
            (Some(id), Some(version), Some(exec_path), Some(home_path)) => Some(JavaRuntime {
                id,
                version,
                exec_path,
                home_path,
                vendor: self.java_vendor,
            }),
            _ => None,
        };

        Settings {
            dir: self.dir,
            java_runtime,
        }
    }
}

#[async_trait]
impl OptionsStore for DbOptionsStore {
    async fn overview(&self, id: InstanceId) -> Result<Overview> {
        let row = sqlx::query_as!(
            OverviewRow,
            r#"
                SELECT i.name, o.tags, o.export_type, o.playtime
                FROM instances_overview o
                JOIN instances i ON i.id = o.instance_id
                WHERE o.instance_id = ?
            "#,
            id
        )
        .fetch_one(&self.db.pool)
        .await?;

        // Значение писали мы сами из закрытого enum'а — нераспознанное
        // означает порчу данных, а не ошибку ввода.
        let export_type: ExportType = row
            .export_type
            .parse()
            .map_err(|_| DbError::ResultCorrupted)?;

        Ok(Overview {
            name: row.name,
            tags: row.tags,
            export_type,
            playtime: row.playtime,
        })
    }

    async fn settings(&self, id: InstanceId) -> Result<Settings> {
        let row = sqlx::query_as!(
            SettingsRow,
            r#"
                SELECT
                    i.dir,
                    j.id        AS "java_id?: i64",
                    j.version   AS "java_version?: String",
                    j.exec_path AS "java_exec_path?: String",
                    j.home_path AS "java_home_path?: String",
                    j.vendor    AS "java_vendor?: String"
                FROM instances_settings s
                JOIN instances i          ON i.id = s.instance_id
                LEFT JOIN java_runtimes j ON j.id = s.java_runtime
                WHERE s.instance_id = ?
            "#,
            id
        )
        .fetch_one(&self.db.pool)
        .await?;

        Ok(row.into_model())
    }

    async fn patch_overview(&self, id: InstanceId, patch: &OverviewPatch) -> Result<()> {
        let export_type = patch.export_type.map(|e| e.to_string());

        sqlx::query!(
            r#"
                UPDATE instances_overview
                SET tags        = COALESCE(?1, tags),
                    export_type = COALESCE(?2, export_type)
                WHERE instance_id = ?3
            "#,
            patch.tags,
            export_type,
            id
        )
        .execute(&self.db.pool)
        .await?;

        Ok(())
    }

    async fn patch_settings(&self, id: InstanceId, patch: &SettingsPatch) -> Result<()> {
        // COALESCE не умеет «выставить NULL»: он для того и существует,
        // чтобы NULL игнорировать. Поэтому присутствие поля в патче
        // передаётся отдельным флагом, а CASE выбирает между записью
        // нового значения и сохранением старого.
        let present = patch.java_runtime.is_some();
        let value = patch.java_runtime.flatten();

        sqlx::query!(
            r#"
                UPDATE instances_settings
                SET java_runtime = CASE WHEN ?1 = 1 THEN ?2 ELSE java_runtime END
                WHERE instance_id = ?3
            "#,
            present,
            value,
            id
        )
        .execute(&self.db.pool)
        .await?;

        Ok(())
    }
}
```

Весь SQL среза теперь в одном файле. Раньше он был размазан по структурам страниц —
`Overview::update`, `Overview::insert`, `Overview::from_db`, `Settings::update`, `Settings::upset`,
`Settings::from_db`, — и каждая из них принимала либо `&Database`, либо `&EndpointRequest`,
по настроению.

---

## Этап 7 — `options/service.rs`

```rust
use std::sync::Arc;

use crate::{
    bus::EventBus,
    data::db::{Database, DbError},
    instance::{
        error::InstanceError,
        model::InstanceId,
        options::{
            error::{OptionsError, Result},
            model::{
                OptionUpdateMessage, Overview, OverviewPatch, PageView, Settings, SettingsPatch,
            },
            storage::{DbOptionsStore, OptionsStore},
        },
        registry::InstanceRegistry,
    },
};

pub struct OptionsService {
    storage: Arc<dyn OptionsStore>,
    instances: Arc<InstanceRegistry>,
    bus: EventBus,
}

impl OptionsService {
    pub fn new(db: Arc<Database>, instances: Arc<InstanceRegistry>, bus: EventBus) -> Self {
        Self {
            storage: Arc::new(DbOptionsStore::new(db)),
            instances,
            bus,
        }
    }

    pub async fn overview(&self, id: InstanceId) -> Result<Overview> {
        self.storage.overview(id).await.map_err(Into::into)
    }

    pub async fn settings(&self, id: InstanceId) -> Result<Settings> {
        self.storage.settings(id).await.map_err(Into::into)
    }

    pub async fn patch_overview(&self, id: InstanceId, patch: OverviewPatch) -> Result<Overview> {
        if patch.is_empty() {
            return self.overview(id).await;
        }

        // Порядок важен. Поля пишутся первыми, переименование последним,
        // потому что NameTaken — единственная реалистичная ошибка этого
        // запроса, и при таком порядке она оставляет имя нетронутым.
        if patch.has_fields() {
            self.storage.patch_overview(id, &patch).await?;
        }

        if let Some(name) = patch.name.as_deref() {
            self.instances
                .rename(id, name)
                .await
                .map_err(map_instance_error)?;
        }

        let view = self.storage.overview(id).await?;
        self.publish(id, PageView::Overview(view.clone()));

        Ok(view)
    }

    pub async fn patch_settings(&self, id: InstanceId, patch: SettingsPatch) -> Result<Settings> {
        if patch.is_empty() {
            return self.settings(id).await;
        }

        self.storage
            .patch_settings(id, &patch)
            .await
            .map_err(|e| match e {
                // FK на java_runtimes(id) уже проверяет существование рантайма,
                // а From<sqlx::Error> for DbError превращает нарушение
                // в DbError::InvalidInput (data/db/mod.rs:85). Предварительная
                // проверка была бы лишним запросом и всё равно гонкой.
                DbError::InvalidInput(_) => match patch.java_runtime.flatten() {
                    Some(java_id) => OptionsError::JavaRuntimeNotFound(java_id),
                    None => OptionsError::Db(e),
                },
                other => OptionsError::Db(other),
            })?;

        let view = self.storage.settings(id).await?;
        self.publish(id, PageView::Settings(view.clone()));

        Ok(view)
    }

    fn publish(&self, instance_id: InstanceId, view: PageView) {
        self.bus.publish(OptionUpdateMessage { instance_id, view });
    }
}

fn map_instance_error(e: InstanceError) -> OptionsError {
    match e {
        InstanceError::NameTaken => OptionsError::NameTaken,
        InstanceError::NotFound(id) => OptionsError::InstanceNotFound(id),
        InstanceError::DB(db) => OptionsError::Db(db),
        other => OptionsError::Internal(other.to_string()),
    }
}
```

Три решения, зафиксированные здесь:

**Патчи возвращают новое состояние, а не `()`.** Вызывающему не нужен второй запрос, чтобы
узнать результат, и то же значение уходит в шину — одно чтение обслуживает и ответ, и событие.

**Пустой патч не пишет и не публикует.** Сейчас `Overview::update` c пустым `OverviewFields`
выполняет `UPDATE` из четырёх `COALESCE(NULL, x)` — то есть запись, которая ничего не меняет.

**Событие несёт полное состояние, а не патч.** Так подписчик не обязан помнить предыдущее
состояние и не зависит от порядка доставки. Это же делает `ScanMessage`.

### Подключение в `data/mod.rs`

```rust
pub struct LauncherServices {
    java: Arc<JavaService>,
    versions: Arc<VersionCatalog>,
    options: Arc<OptionsService>,
}
```

`OptionsService::new` требует `Arc<InstanceRegistry>`. Сейчас registry создаётся внутри
`InstanceService::new` и наружу не отдаётся — его нужно поднять на уровень `GlobalState::new` и
передать в оба сервиса, иначе у options и instance будут разные кэши одного и того же.

---

## Этап 8 — `options/dto.rs`

Единственное место среза, знающее про JSON и про раскладку страниц во фронтенде.

```rust
use std::str::FromStr;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::instance::{
    model::InstanceId,
    options::{
        error::{OptionsError, Result},
        model::{OverviewPatch, SettingsPatch},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Settings,
}

impl FromStr for Page {
    type Err = OptionsError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "overview" => Ok(Page::Overview),
            "settings" => Ok(Page::Settings),
            other => Err(OptionsError::UnknownPage(other.to_string())),
        }
    }
}

#[derive(Debug)]
pub enum PagePatch {
    Overview(OverviewPatch),
    Settings(SettingsPatch),
}

/// Сырое тело запроса: страница ещё строка, патч ещё не разобран.
#[derive(Debug, Deserialize)]
pub struct ChangeRequestBody {
    id: i64,
    page: String,
    options: Box<RawValue>,
}

#[derive(Debug)]
pub struct ChangeRequest {
    pub id: InstanceId,
    pub patch: PagePatch,
}

impl ChangeRequestBody {
    /// RawValue позволяет отложить разбор `options` до тех пор,
    /// пока не известна страница — иначе пришлось бы держать
    /// объединённую структуру со всеми полями всех страниц.
    pub fn build(self) -> Result<ChangeRequest> {
        let page: Page = self.page.parse()?;

        let patch = match page {
            Page::Overview => PagePatch::Overview(serde_json::from_str(self.options.get())?),
            Page::Settings => PagePatch::Settings(serde_json::from_str(self.options.get())?),
        };

        Ok(ChangeRequest {
            id: InstanceId::from(self.id),
            patch,
        })
    }
}
```

`match page` стал исчерпывающим: пять страниц-заглушек удалены, ветки `_ =>` и
`InstanceError::OptionsPageWrong` больше не нужны.

`tide` здесь не появляется — DTO принимает уже разобранный JSON, а не `Request`. HTTP-обработчик,
когда его включат, будет в три строки:

```rust
// endpoint/instance.rs, когда mod endpoint вернут в main.rs
let body: ChangeRequestBody = req.body_json().await?;
let request = body.build()?;
let view = match request.patch {
    PagePatch::Overview(p) => PageView::Overview(options.patch_overview(request.id, p).await?),
    PagePatch::Settings(p) => PageView::Settings(options.patch_settings(request.id, p).await?),
};
```

### Про `[features]`

Секции `[features]` в `Cargo.toml` сейчас нет вообще, поэтому `dto.rs` компилируется безусловно.
Зависит он только от `serde`/`serde_json`, так что навесить `#[cfg(feature = "server")]` при
разделении — правка одной строки.

Заводить фичу сейчас ради одного файла не стоит: `tide` остаётся безусловной зависимостью крейта
(его тянет даже `async_trait` в storage-файлах), и получится видимость разделения без разделения.
Настоящее разделение — отдельная работа: `tide::utils::async_trait` во всех срезах меняется на
`async-trait`, `endpoint/` и `websocket/` уезжают под фичу целиком.

---

## Тесты

### `storage.rs` — интеграционные, на in-memory БД

```rust
async fn test_db() -> Arc<Database> {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    Arc::new(Database { pool })
}
```

Миграции гоняются в памяти — файл не нужен, `dev.db` не трогается.

| Тест | Ожидание |
|---|---|
| `overview_reads_name_from_instances` | имя приходит из `instances`, а не из `instances_overview` |
| `settings_without_java_runtime` | `java_runtime IS NULL` → `None`, а не ошибка |
| `settings_with_java_runtime` | `LEFT JOIN` собирает полный `JavaRuntime` |
| `patch_settings_clears_runtime` | `Some(None)` записывает NULL |
| `patch_settings_keeps_runtime` | `None` не трогает существующее значение |
| `patch_settings_rejects_unknown_runtime` | FK-нарушение → `DbError::InvalidInput` |
| `patch_overview_partial` | `COALESCE` не затирает поля, которых нет в патче |

### `service.rs` — на фейковом сторе

| Тест | Ожидание |
|---|---|
| `patch_publishes_once_with_new_state` | ровно одно событие, в нём состояние после записи |
| `empty_patch_writes_nothing` | ни записи, ни события |
| `rename_to_taken_name_maps_to_name_taken` | `DbError::Conflict` → `OptionsError::NameTaken` |
| `unknown_java_runtime_maps_to_typed_error` | `DbError::InvalidInput` → `JavaRuntimeNotFound(id)` |

События собираются через `EventBus::subscribe()`.

### `dto.rs` — переезжают из `tests.rs` как есть

Пять существующих тестов (`parse_change_request_partially`, `_fully`, `_empty`, `_wrong_page`,
`parse_change_requets_different_case`) проверяют именно диспетчеризацию и остаются рабочими после
переименования `ChangeRequestBuilder` → `ChangeRequestBody` и `ChangableOptions` → `PagePatch`.

Добавить к ним:

| Тест | Ожидание |
|---|---|
| `settings_patch_distinguishes_null_and_absent` | `{"java_runtime": null}` → `Some(None)`, `{}` → `None` |
| `unknown_field_is_rejected` | `deny_unknown_fields` работает: `{"playtime": 9999}` в `OverviewPatch` → ошибка |

Второй заодно фиксирует решение убрать `playtime` из патча.

---

## Что переиспользуем, а не пишем заново

| Уже есть | Где | Роль |
|---|---|---|
| `JavaStore` / `DbJavaStore` | `java/storage.rs` | Образец для `OptionsStore` + `DbOptionsStore` |
| `JavaService` | `java/service.rs` | Образец для `OptionsService` |
| `InstanceStore::update_name` | `instance/storage.rs:84` | Запись имени, сейчас не вызывается |
| `InstanceRegistry::upsert` | `instance/registry.rs:81` | Обновление кэша после переименования |
| `EventBus::publish` / `subscribe` | `bus/mod.rs` | Публикация `OptionUpdateMessage` |
| `CoreEvent::Option` | `bus/event.rs:9` | Вариант шины уже заведён |
| `DbError` | `data/db/mod.rs` | `Conflict` для UNIQUE, `InvalidInput` для FK — распознавание уже написано |
| `ScanMessage` | `instance/scan.rs` | Образец payload-only доменного сообщения |

---

## Верификация

**Компиляция.** После каждого этапа `cargo check`. Ориентир по срезу — шесть ошибок сейчас:
`E0433` (`crate::websocket`), `E0425` (`BaseMessage`), два `E0609` (`static_data`), `E0599`
(`update_instance_name`) и три sqlx-ошибки на несуществующие колонки `name`/`dir`.

Макросы sqlx открывают БД на запись, поэтому проверять нужно с доступной `dev.db`:

```bash
DATABASE_URL="sqlite:$PWD/dev.db" cargo check
```

**Отсутствие следов старого среза.**

```bash
grep -rn "EndpointRequest" src/instance/                    # пусто
grep -rn "crate::websocket" src/instance/                   # пусто
grep -rn "ChangableOptions\|PageResult\|ReadPage" src/       # пусто
grep -rn "OptionsPageWrong\|OptionNotAvailable" src/         # пусто
grep -rn "InstanceFields" src/                               # пусто
ls src/instance/options/pages 2>/dev/null                    # нет каталога
ls src/websocket/messages/option 2>/dev/null                 # нет каталога
```

**Сквозной прогон.**

- `InstanceService::create` → в БД появляются три строки:
  `SELECT (SELECT COUNT(*) FROM instances_overview), (SELECT COUNT(*) FROM instances_settings);`
- чтение overview сразу после создания отдаёт дефолты (`tags = ''`, `export_type = 'Sonata'`,
  `playtime = 0`) и имя инстанса — а не `RowNotFound`, как сейчас;
- патч с `name` меняет имя и в `instances`, и в том, что отдаёт `InstanceRegistry::get` —
  без перезапуска;
- патч с занятым именем возвращает `NameTaken`, и `tags` из того же запроса при этом применены
  (осознанное следствие неатомарности, см. Context);
- патч `{"java_runtime": null}` обнуляет колонку, `{}` — не трогает;
- `SELECT java_runtime FROM instances_settings` после удаления рантайма даёт NULL
  (`ON DELETE SET NULL`, заведён в `REFACTOR_DOWNLOAD_IMPL.md`, этап 6);
- на каждый непустой патч в `EventBus` приходит ровно одно `CoreEvent::Option`.
