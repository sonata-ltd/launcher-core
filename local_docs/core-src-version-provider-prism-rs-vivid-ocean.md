# Архитектура Prism-провайдера в `src/version/`

## Context

`src/version/` — новый непроиндексированный (`?? src/version/`) модуль, который должен заменить три
параллельные реализации одной и той же задачи: `src/utils/unify/mod.rs`, `src/manifest/mod.rs` и
захардкоженные URL в `src/endpoint/versions.rs`. Ядро модуля — `VersionCatalog` (кэш) и
`MetaProvider` (источник метаданных). Заготовка `src/version/provider/prism.rs` **не компилируется**
и по факту не использует объявленные в ней типы.

Цель документа — зафиксировать целевую архитектуру парсинга Prism-манифеста, границы текущей
итерации и порядок работ. Код здесь намеренно ограничен сигнатурами и формами типов — реализация
за вами.

---

## 1. Что конкретно сломано в `src/version/provider/prism.rs`

Файл содержит и мёртвые типы, и ручной обход `serde_json::Value` — две несовместимые стратегии
одновременно. Ошибки по строкам:

| Строка | Проблема |
|---|---|
| 13 | `type: PrismVersionKind` — `type` зарезервированное слово. Нужен `#[serde(rename = "type")] kind: …`. Жёсткая ошибка компиляции. |
| 11–12 | `PrismVersion` выводит только `Debug`, но используется как `Vec<PrismVersion>` внутри `#[derive(Deserialize)]`-структуры → ошибка даже после починки (13). |
| 5–25 | `PrismIndex`/`PrismVersion`/`PrismVersionKind` объявлены, но **нигде не используются** — `parse_prism` игнорирует их и ковыряет `Value` руками. |
| 24 | `Experiement` — опечатка (`Experiment`). |
| 17–25 | У `PrismVersionKind` нет `#[serde(rename_all = "snake_case")]`, в отличие от `VersionKind`. Реальные значения в манифесте — `old_snapshot`, а не `OldSnapshot`. |
| 39 | `serde_json::from_str(kind)` где `kind` — голая строка `release`. Это невалидный JSON, вызов упадёт в рантайме. Нужен `from_value`, а не `from_str`. |
| 41–43 | `let releaed_at: Option<DateTime<Utc>> = … .and_then(\|v\| v.as_str());` — несовпадение типов (`Option<&str>` vs `Option<DateTime<Utc>>`), сам парсинг закомментирован. |
| 41 vs 48 | Опечатка в имени: объявлено `releaed_at`, используется `released_at` → переменная не определена. |
| 1–2 | `use super::*;` уже втягивает `MetaProvider`; строка 2 — дубль импорта. |
| 32–54 | Каждый `if let Some(...)` без `else`: битый манифест даёт `Ok(vec![])`, а не ошибку. Тихая потеря данных. |

**Семантическая дыра, важнее всех перечисленных.** `VersionKind` в `provider/mod.rs:26-31`
содержит только `Release | Snapshot | OldBeta | OldAlpha`. Реальный набор в Prism (проверено на
живом индексе, 1009 записей):

```
snapshot      740
release       102
old_snapshot   75    <-- отсутствует в VersionKind
old_alpha      35
experiment     31    <-- отсутствует в VersionKind
old_beta       26
```

То есть **106 из 1009 версий (10,5%)** сейчас либо провалят парсинг, либо будут молча выброшены.

---

## 2. Факты о манифесте (проверено на живом endpoint'е)

### `https://meta.prismlauncher.org/v1/net.minecraft/index.json`

Размер ~436 КБ, 1009 записей. Корень: `formatVersion`, `name`, `uid`, `versions`.

У **всех 1009 записей присутствуют все шесть полей** — ни одно не опционально на практике:

```json
{
  "recommended": false,
  "releaseTime": "2026-07-21T11:45:42+00:00",
  "requires": [ { "suggests": "3.4.2", "uid": "org.lwjgl3" } ],
  "sha256": "730f6bd3…",
  "type": "snapshot",
  "version": "26.3-snapshot-5"
}
```

Ключевые следствия:

- `releaseTime` — **RFC 3339** со смещением `+00:00`. Парсить `DateTime::parse_from_rfc3339(...)`
  → `.with_timezone(&Utc)`. Не `Utc.datetime_from_str`.
- Индекс отсортирован **от новых к старым**. Не полагайтесь на это — гарантию порядка должен
  задавать доменный слой явно.
- `sha256` — хэш **файла деталей версии** (`net.minecraft/<version>.json`). Это готовый механизм
  инвалидации дискового кэша, если вы к нему придёте.
- `recommended` — то, чем Prism помечает «latest release». Без него UI не сможет показать
  рекомендованную версию, а `VersionSummary` это поле сейчас теряет.
- `requires` — граф зависимостей компонентов. Для ванильного Minecraft это всегда `org.lwjgl3`
  (или `org.lwjgl` на старых версиях). Это **не** декоративное поле: файл
  `net.minecraft/<v>.json` неполон, натив-библиотеки LWJGL лежат в отдельном компоненте.

### `https://meta.prismlauncher.org/v1/index.json` (корневой)

`{ formatVersion, packages: [ { name, sha256, uid } ] }` — реестр всех компонентов:
`net.minecraft`, `org.lwjgl3`, `net.fabricmc.fabric-loader`, `net.minecraftforge`,
`net.adoptium.java`, и т.д. Пригодится, когда дойдёте до модлоадеров.

### `https://meta.prismlauncher.org/v1/net.minecraft/<version>.json`

Ключи (на примере 1.21.4): `+traits`, `assetIndex`, `compatibleJavaMajors`, `compatibleJavaName`,
`formatVersion`, `libraries`, `logging`, `mainClass`, `mainJar`, `minecraftArguments`, `name`,
`order`, `releaseTime`, `requires`, `type`, `uid`, `version`.

Отличия от манифеста Mojang, которые уже отражены в коде:

| Mojang | Prism | Где обрабатывается |
|---|---|---|
| `id` | `version` | `instance/init.rs:50-64` (ручной фолбэк) |
| `downloads.client` | `mainJar.downloads.artifact` | `libs/parse/prism.rs:154` |
| всё в одном файле | граф `requires` | `libs/parse/prism.rs:115-149` (рекурсия) |
| — | `+traits`, `order`, `compatibleJavaMajors` | нигде |

`assetIndex` присутствует в обоих форматах одинаково — `get_assets_manifest`
(`instance/download/manifest.rs:48`) работает для обоих без изменений.

---

## 3. Рекомендуемая архитектура

### 3.1 Три слоя вместо одного

Сейчас `parse_prism` смешивает транспорт (сборку URL), формат провайдера и доменную модель.
Разделите:

```
слой 1  DTO провайдера   — точное зеркало wire-формата, private для модуля prism
слой 2  конверсия        — TryFrom<Dto> for VersionSummary, здесь живёт вся нормализация
слой 3  доменная модель  — VersionSummary / VersionKind, ничего не знает про Prism
```

**Слой 1** — типизированный `serde`, а не `Value`-ковыряние:

```rust
// src/version/provider/prism.rs — всё private
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismIndex {
    format_version: u32,
    uid: String,
    name: String,
    versions: Vec<PrismIndexEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismIndexEntry {
    version: String,
    #[serde(rename = "type")]
    kind: PrismVersionKind,
    release_time: DateTime<Utc>,   // chrono умеет RFC3339 из коробки
    sha256: String,
    recommended: bool,
    #[serde(default)]
    requires: Vec<PrismRequirement>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismRequirement {
    uid: String,
    suggests: Option<String>,
    equals: Option<String>,   // встречается у forge/fabric-компонентов
}
```

Почему DTO, а не `Value`: `serde` даёт вам позицию и имя поля в сообщении об ошибке бесплатно,
а ручной обход даёт `Ok(vec![])`. На 1009 записях разница между «упало на записи 812, поле
releaseTime» и «пустой список» — это часы отладки.

**Не ставьте `#[serde(deny_unknown_fields)]`.** Prism добавляет поля в манифест без смены
`formatVersion`; строгий режим превратит любое их обновление в отказ вашего лаунчера.

### 3.2 `VersionKind` должен быть объединением всех провайдеров + escape hatch

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionKind {
    Release,
    Snapshot,
    OldSnapshot,   // добавить: 75 записей
    OldBeta,
    OldAlpha,
    Experiment,    // добавить: 31 запись
    #[serde(other)]
    Unknown,       // добавить: страховка от новых типов
}
```

`#[serde(other)]` здесь не косметика. Индекс — живой upstream-фид, который вы не контролируете.
Без варианта-заглушки один новый `type` со стороны Prism кладёт весь список версий, а не одну
запись. С заглушкой — версия просто отображается как неизвестный тип.

`PrismVersionKind` из заготовки после этого не нужен вообще: он в точности повторяет `VersionKind`.
Уберите его и десериализуйте `kind` сразу в `VersionKind`. Отдельный DTO-enum оправдан только если
у провайдеров разойдутся имена значений — у Prism и Mojang они совпадают.

### 3.3 `VersionSummary` теряет данные, которые нужны выше по стеку

```rust
#[derive(Debug, Clone, Serialize)]
pub struct VersionSummary {
    pub id: String,
    pub url: String,
    pub kind: VersionKind,
    pub released_at: Option<DateTime<Utc>>,
    // добавить:
    pub provider: MetaProvider,   // см. §4 — убирает хардкод в init.rs:70
    pub recommended: bool,        // Mojang: id == latest.release
    pub sha256: Option<String>,   // Prism даёт, Mojang нет
}
```

- `provider` — самое важное дополнение. Без него потребитель (`instance/init.rs`) не знает, каким
  парсером разбирать скачанный по `url` файл, и хардкодит `ManifestType::Prism`.
- `Serialize` отсутствует у `VersionSummary` сейчас — а модуль по определению отдаёт данные в API.
  Добавьте, и если версии пойдут в WebSocket — `#[derive(TS)]` с `#[ts(export_to = "./versions/")]`
  по образцу `src/instance/options/pages/settings.rs:23`.
- `released_at` оставьте `Option`: в Prism поле есть всегда, но доменный тип обслуживает и другие
  источники.

### 3.4 Enum-как-ключ + trait-как-поведение

Сейчас `MetaProvider` совмещает две роли: идентификатор (ключ `HashMap` в кэше — отсюда
`Hash + Eq + Copy`) и носитель поведения (`parse_index` через `match`). Вторая роль плохо
масштабируется: добавление провайдера = новый вариант + 4 руки `match` + новый `impl`-блок в
отдельном файле. Логика провайдера размазана по `provider/mod.rs` и `provider/<name>.rs`.

Разделите роли, сохранив дешёвый ключ:

```rust
// идентификатор — остаётся Copy + Hash + Eq, годится как ключ кэша
pub enum MetaProvider { Mojang, Prism }

impl MetaProvider {
    pub fn spec(&self) -> &'static dyn VersionSource { … }   // единственный match
}

pub trait VersionSource: Send + Sync {
    fn id(&self) -> MetaProvider;
    fn index_url(&self) -> &'static str;
    fn parse_index(&self, raw: &[u8]) -> Result<Vec<VersionSummary>>;
}

pub struct PrismSource;   // impl VersionSource — вся Prism-логика в одном файле
```

Выигрыш: `index_url_base()` перестаёт быть публичным методом с `todo!()` для Mojang (сейчас
`provider/mod.rs:63` — гарантированная паника, если кто-то выберет Mojang); сборка URL становится
приватной деталью `PrismSource`; добавление провайдера = один новый файл + одна строка в `spec()`.

Если trait кажется преждевременным — минимум, который стоит сделать в любом случае: убрать
`todo!()` из `index_url_base()` и `parse_index()`, заменив на
`Err(VersionError::ProviderNotSupported(*self))`. `todo!()` в коде, доступном через API, — это
паника на пользовательском вводе.

### 3.5 Парсите из байтов, а не из `Value`

`catalog.rs:38-42` сейчас делает: `download_in_json` → полный DOM `Value` (436 КБ, ~1009 объектов
с вложенными массивами) → `parse_index(&Value)` → снова обход и аллокация строк. Двойной проход и
пиковое потребление памяти в разы выше необходимого.

Замените на `download(url) -> Vec<u8>` (уже есть в `utils/download/mod.rs:16`) +
`serde_json::from_slice::<PrismIndex>(&bytes)`. Отсюда сигнатура `parse_index(&self, raw: &[u8])`
в §3.4. Побочный бонус: провайдер получает доступ к сырым байтам и может считать sha256 для
дискового кэша.

---

## 4. Границы этой итерации — и почему именно такие

Решение: **провайдер отвечает только за индекс (список версий), детали версии остаются
`serde_json::Value`.** Обоснование — из `src/instance/init.rs`, единственного реального
потребителя:

1. **`init.rs:33`** — `download_manifest(&client_data.url, Some(paths.meta()))`. Всё, что нужно от
   version-слоя, — это `url`. Ровно `VersionSummary.url`. Типизированный `VersionDetail` тут не
   требуется.
2. **`init.rs:50-64`** — ручной фолбэк `manifest["id"]` → `manifest["version"]`. Это симптом
   отсутствия нормализации: доменный слой уже знает id, потому что сам его распарсил из индекса.
   Передавайте `summary.id` в `install()` и удаляйте эти 15 строк — не читайте id заново из файла.
3. **`init.rs:70`** — `ManifestType::Prism` захардкожен. С полем `VersionSummary.provider` это
   становится `summary.provider.into()`.
4. **`LibsData::sync_libs`** и `AssetsData::sync_assets` принимают `&serde_json::Value` и уже
   имеют рабочие парсеры для обоих форматов (`libs/parse/mod.rs` для Mojang, `libs/parse/prism.rs`
   для Prism). Типизация деталей версии означала бы переписывание обоих парсеров + assets —
   отдельный крупный рефакторинг.

Что **не** входит в итерацию и почему:

- **Типизированный `VersionDetail`** — нужен только когда будете переписывать `libs/parse/*`.
  Делать заранее — значит поддерживать два представления одних данных.
- **Резолв графа `requires`** — уже реализован в `libs/parse/prism.rs:115-149` (рекурсивно, через
  `Box::pin`). Работает. Переносить его в `version::` имеет смысл вместе с §«модлоадеры», не
  раньше. Отметьте только, что там свой захардкоженный `META_BASE_URL` (`prism.rs:15`) —
  третья копия того же URL.

---

## 5. Проблемы в `catalog.rs`, которые стоит починить заодно

| Место | Проблема | Что делать |
|---|---|---|
| `catalog.rs:68` | `set_default_provider(&mut self)` **заменяет** `Mutex` целиком вместо блокировки. Метод требует `&mut self`, поэтому непригоден за `Arc` — а все остальные методы берут `&self`. | `&self` + `*self.default_provider.lock().unwrap() = provider` |
| `catalog.rs:33` | Нет single-flight: N одновременных промахов кэша = N скачиваний по 436 КБ. При старте UI это типичный сценарий. | Дедупликация: `HashMap<MetaProvider, Shared<BoxFuture<…>>>` либо per-provider async-мьютекс вокруг фетча |
| `catalog.rs:14-15` | `std::sync::Mutex` в async-коде, тогда как весь остальной новый код (`java/registry.rs:14`) использует `async_std::sync::RwLock`. Сейчас лок не удерживается через `.await`, поэтому не дедлочит — но это хрупко. | Привести к `async_std::sync::RwLock` для единообразия |
| `catalog.rs:73,81,85,95` | `.unwrap()` на `lock()` — паника при отравлении мьютекса | Уйдёт вместе с переходом на `RwLock` |
| `catalog.rs:48-58` | `resolve_from` — линейный скан по 1009 элементам на каждый вызов | Если станет горячим: `HashMap<String, usize>` рядом с `versions` в `CachedIndex` |
| `catalog.rs:49` | `provider.clone()` на `Copy`-типе | clippy: `clone_on_copy` |
| `catalog.rs:38` | `&provider.index_url()` — двойная ссылка на `&'static str` | убрать `&` |
| `catalog.rs:23` | TTL 6 ч только в памяти: каждый перезапуск процесса = 436 КБ по сети | Дисковый кэш в `paths.meta()`, валидация через `sha256` из корневого `/v1/index.json`. `data/layout.rs` сейчас не имеет каталога под meta-кэш — потребуется добавить |

`VersionError` (`error.rs`): `IndexUnavailable(String)` теряет HTTP-статус surf-ошибки, а
`SerializationFailed(#[from] serde_json::Error)` не говорит, *какой* провайдер и *какой* документ
не разобрался. Добавьте контекст:

```rust
#[error("malformed index from {provider}: {source}")]
MalformedIndex { provider: MetaProvider, #[source] source: serde_json::Error },

#[error("provider {0} is not supported yet")]
ProviderNotSupported(MetaProvider),
```

---

## 6. Дублирующие реализации: план миграции

Сейчас Prism-парсер существует в трёх местах, Mojang-URL — в четырёх:

| Файл | Что делает | Судьба |
|---|---|---|
| `src/utils/unify/mod.rs` | `UnifiedVersionsData` + `MetaProviders` (мн. ч.!) — прямой предок `parse_prism`, `extract_prism():56`. Без `kind`, без дат, без кэша. | Удалить после перевода `endpoint/versions.rs` |
| `src/manifest/mod.rs` | `get_global_manifest()` / `get_version_manifest(id)` — Mojang, `Result<_, String>` | Поглощается `VersionCatalog::resolve` |
| `src/endpoint/versions.rs:32` | `get_versions_unified` → `UnifiedVersionsData` | Перевести на `VersionCatalog` |
| `src/endpoint/versions.rs:49-58` | `get_versions` — третья копия обоих URL | Перевести на `MetaProvider::index_url()` |
| `src/instance/download/libs/parse/prism.rs:15` | `META_BASE_URL` — четвёртая копия base-URL | Взять из провайдера |

Порядок безопасной миграции (важно: `mod endpoint;` в `main.rs:31` **закомментирован**, а
`main.rs:33` объявляет несуществующий `mod operation;` — крейт сейчас не собирается, это надо
учитывать при планировании шагов):

1. Довести `src/version/` до компилируемого и покрытого тестами состояния **изолированно** — он ни
   от чего не зависит и ни на что не влияет.
2. Подключить `VersionCatalog` в `GlobalState` (`data/mod.rs:33-40`) — естественное место
   `LauncherServices` (`data/mod.rs:23`), сейчас пустая структура с закомментированным
   `java: Arc<JavaService>`.
3. Перевести `endpoint/versions.rs` на каталог, удалить `utils/unify`.
4. Перевести `instance/init.rs` на `VersionSummary` (§4, пункты 1–3), удалить фолбэк `id`/`version`.
5. `manifest/mod.rs` и `META_BASE_URL` в `libs/parse/prism.rs` — последними.

---

## 7. Тесты

В `src/version/` тестов нет вообще. Прецеденты в проекте: `src/utils/download/tests.rs`,
`src/instance/options/tests.rs`.

Заведите офлайн-фикстуру и не ходите в сеть из тестов:

- `tests/fixtures/prism_index.json` — урезанный индекс, ~15 записей, обязательно покрывающий **все
  шесть** реальных значений `type` плюс одну запись с выдуманным `type` (проверка
  `#[serde(other)]`).
- `tests/fixtures/prism_index_malformed.json` — отсутствующий `versions`, битый `releaseTime`.

Что проверять:

1. Все шесть `type` маппятся в правильные `VersionKind` (сейчас 10,5% индекса потерялось бы молча).
2. Неизвестный `type` → `VersionKind::Unknown`, остальные записи целы.
3. `releaseTime` `"2026-07-21T11:45:42+00:00"` парсится в `DateTime<Utc>`.
4. URL собирается как `https://meta.prismlauncher.org/v1/net.minecraft/26.2.json` — с `uid` из
   документа, а не из константы.
5. Битый конверт (`versions` отсутствует) → `Err(MalformedIndex)`, **не** `Ok(vec![])`.
6. `recommended: true` доезжает до `VersionSummary`.

---

## 8. Порядок работ

1. `provider/mod.rs`: расширить `VersionKind` (`OldSnapshot`, `Experiment`, `#[serde(other)] Unknown`);
   расширить `VersionSummary` (`provider`, `recommended`, `sha256`, `Serialize`);
   убрать `todo!()` в пользу `ProviderNotSupported`.
2. `error.rs`: добавить `MalformedIndex { provider, source }` и `ProviderNotSupported`.
3. `provider/prism.rs`: выбросить весь текущий контент; DTO по §3.1 + `TryFrom`-конверсия;
   парсинг из `&[u8]`.
4. Тесты по §7 на фикстурах.
5. `catalog.rs`: `set_default_provider(&self)`, `RwLock`, single-flight, `from_slice` вместо `Value`.
6. (опционально, отдельным шагом) trait `VersionSource` по §3.4.
7. Миграция потребителей по §6.

## Верификация

- `cargo check -p core` и `cargo clippy` — крейт сейчас не собирается по независимым причинам
  (`main.rs:33` объявляет несуществующий `mod operation;`), поэтому изолированно проверяйте
  модуль через `cargo test version::` после того, как сборка будет починена.
- `cargo test version::` — офлайн, на фикстурах.
- Разовая ручная проверка против живого индекса (не в CI): скачать
  `https://meta.prismlauncher.org/v1/net.minecraft/index.json`, распарсить, убедиться что
  `versions.len() == 1009` (на 2026-07-27) и что ни одна запись не свалилась в `Unknown`.
- Спот-чек URL: взять любой `VersionSummary.url` из результата и убедиться, что он отдаёт 200.
