# Архитектурные рекомендации

Отвечу на оба вопроса по порядку.

---

# 1. Да, ваша интуиция верна — и это идиоматичный путь в Rust

Держать всё, что связано с `instance`, внутри модуля `instance` — правильно.

В §5.2 я дал два варианта, и второй ("при переходе на вертикальные срезы всё станет `instance/model.rs` + `instance/registry.rs`") — это именно ваш вариант, и в §6.5 он назван целевым.

Первый вариант (перенести в `data/registry`) — лишь минимальная заплатка, если текущую структуру не трогать: она чинит разорванный `impl`, но не чинит владение.

Раз вы готовы реорганизовать — забудьте про первый вариант.

## Почему группировка по домену, а не по техническому слою — путь, принятый в Rust

### Приватность

Приватность в Rust работает по модулям.

Если `InstanceDataRow`, его реестр и загрузка из БД живут в одном модуле `instance`, поля и внутренние помощники могут быть `pub(crate)` или полностью приватными, а наружу торчит только осмысленный API.

Когда тип находится в `instance/list.rs`, а `impl` — в `data/registry/instance/`, всё вынужденно становится `pub`, и инкапсуляция исчезает.

---

### `impl` рядом с типом

Технически внутри одного crate можно писать `impl` где угодно.

Но идиома Rust — держать их рядом с объявлением типа.

Разнесённый `impl` обычно встречается только у трейтов с блоками:

```rust
impl Trait for Type
```

которые находятся в модуле самого трейта.

---

### Направление зависимостей

Сейчас получается так:

```
data/registry/instance
        │
        ▼
crate::instance::list::InstanceDataRow
```

То есть инфраструктурный слой зависит от доменного.

После перехода к вертикальному срезу зависимость становится правильной:

```
instance/registry.rs
        │
        ▼
data/
(Database, Registry, ...)
```

Доменный модуль использует инфраструктуру, а не наоборот.

---

### В проекте уже есть такой пример

У вас уже существует хороший образец — модуль `java`:

- `model.rs`
- `error.rs`
- `service.rs`
- `storage.rs`
- `registry.rs`

Точно так же должен выглядеть `instance`.

## Целевая структура

```text
src/instance/
├── mod.rs        # публичный фасад
├── model.rs      # Instance, InstanceDataRow, InitData, RunData
├── error.rs      # InstanceError
├── service.rs    # InstanceService: init/run/get_page/change_field
├── storage.rs    # trait InstanceStore + DbInstanceStore
├── registry.rs   # реестр живых инстансов + broadcast
├── download/
├── launch/
└── options/
```

В `data/` остаётся только то, что действительно является общей инфраструктурой:

- `Database`
- `Config`
- общий `Registry<K, V>` (если реализуете вариант из §3.3)

---

# 2. Как отвязать `WebSocketConnection` от слоёв

Здесь есть два разных случая.

---

## Случай А — функциям WebSocket вообще не нужен

Например:

- `Overview::update`
- `Settings::update`
- `Options::change`

Они используют из `EndpointRequest` только:

- `state`
- `database`

Поэтому вместо этого:

```rust
pub async fn update(
    change: OverviewFields,
    req: &EndpointRequest<'_>,
    instance_id: i64,
) -> Result<()> {
    let db = &req.state().static_data.db;

    req.state().update_instance_name(...).await;

    sqlx::query!(...)
        .execute(&db.pool)
        .await?;

    ...
}
```

делаем так:

```rust
pub async fn update(
    change: OverviewFields,
    state: &GlobalDataState<'_>,
    instance_id: i64,
) -> Result<()> {
    let db = &state.static_data.db;

    state.update_instance_name(...).await;

    sqlx::query!(...)
        .execute(&db.pool)
        .await?;

    ...
}
```

---

### Тогда endpoint становится единственным местом, которое знает про Tide

```rust
pub async fn instance_option_change(
    mut req: EndpointRequest<'_>,
) -> tide::Result {

    let request: ChangeRequestBuilder =
        req.body_json().await?;

    Instance::change_field(
        req.state(),
        request.build()?,
    )
    .await?;

    Ok(
        tide::Response::builder(200).build()
    )
}
```

Получается простая схема:

```
HTTP
    │
Endpoint
    │
GlobalState
    │
Domain
```

Домен больше не знает про Tide.

---

# Случай Б — `Instance::init` шлёт прогресс в WebSocket

Это уже главный случай.

Сейчас происходит следующее:

```
Instance::init
      │
      ├── пишет в WebSocket
      └── обновляет Tasks
```

То есть домен знает сразу о двух потребителях.

Правильнее сделать наоборот.

---

## Идея

Домен не отправляет сообщения.

Он публикует события.

А уже вызывающий код решает, куда их доставить.

Для этого отлично подходит `async-broadcast`.

---

# Шаг 1. OperationHandle

```rust
use async_broadcast::{broadcast, Receiver, Sender};

pub struct OperationHandle {
    tx: Sender<OperationMessage>,
    base: BaseMessage,
}

impl OperationHandle {
    pub fn begin(
        request_id: Option<String>,
        stages: Vec<OperationStage>,
    ) -> (Self, Receiver<OperationMessage>) {

        let (mut tx, rx) = broadcast(64);
        tx.set_overflow(true);

        let handle = Self {
            tx,
            base: BaseMessage::new(request_id),
        };

        handle.emit(
            OperationStart { stages }.into()
        );

        (handle, rx)
    }

    pub fn start_stage(
        &self,
        stage: OperationStage,
    ) {
        self.emit(
            OperationUpdate::Indeterminable {
                stage,
                status: ProcessStatus::Started,
            }
            .into(),
        );
    }

    pub fn update(
        &self,
        stage: OperationStage,
        target: Option<ProcessTarget>,
        current: usize,
        total: usize,
    ) {
        self.emit(
            OperationUpdate::Determinable {
                stage,
                status: ProcessStatus::InProgress,
                target,
                current,
                total,
            }
            .into(),
        );
    }

    pub fn complete_stage(
        &self,
        stage: OperationStage,
        status: StageStatus,
    ) {
        self.emit(
            OperationUpdate::Completed(
                StageResult {
                    status,
                    stage,
                    duration_secs: 0.0,
                    error: None,
                },
            )
            .into(),
        );
    }

    fn emit(&self, data: OperationEvent) {
        let _ = self.tx.try_broadcast(
            OperationMessage {
                base: self.base.clone(),
                data,
            },
        );
    }
}
```

---

# Шаг 2. Домен получает `OperationHandle`

Было:

```rust
init(..., req, ws)
```

Станет:

```rust
impl InstanceService {
    pub async fn init(
        &self,
        data: InitData,
        register: bool,
        op: &OperationHandle,
    ) -> Result<(Instance, LaunchInfo)> {

        op.start_stage(
            OperationStage::FetchManifest
        );

        let manifest =
            download_manifest(...).await?;

        op.complete_stage(
            OperationStage::FetchManifest,
            StageStatus::Completed,
        );

        op.start_stage(
            OperationStage::DownloadLibs
        );

        LibsData::sync_libs(
            &manifest,
            &paths,
            op,
            &self.db,
            ...
        )
        .await?;

        ...
    }
}
```

Теперь домен знает только про `OperationHandle`.

Никаких WebSocket.

---

## Побочный плюс

Исчезает двойная бухгалтерия.

Раньше:

```
OperationWsMessage(...)
update_task(...)
```

Теперь:

```
OperationHandle
       │
       ▼
канал
       │
       ├── WebSocket
       ├── Tasks
       └── GUI
```

---

# Шаг 3. Сервер становится ретранслятором

```rust
pub async fn init_instance_ws(
    req: EndpointRequest<'_>,
    mut ws: WebSocketConnection,
) -> tide::Result<()> {

    while let Some(Ok(Message::Text(input))) =
        ws.next().await
    {
        let data: InitData =
            serde_json::from_str(&input)?;

        let (op, mut events) =
            OperationHandle::begin(
                Some(data.request_id.clone()),
                vec![
                    OperationStage::FetchManifest,
                    OperationStage::DownloadLibs,
                    OperationStage::DownloadAssets,
                ],
            );

        let relay = task::spawn({
            let ws = ws.clone();

            async move {
                while let Ok(msg) =
                    events.recv().await
                {
                    if ws
                        .send_json(
                            &json!(WsMessage::Operation(msg)),
                        )
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });

        let result =
            req.state()
                .instance_service()
                .init(data, true, &op)
                .await;

        drop(op);

        relay.await;

        ws.send_json(
            &json!(response_from(result)),
        )
        .await?;
    }

    Ok(())
}
```

Получается очень тонкая прослойка:

```
Domain
    │
Channel
    │
Relay
    │
WebSocket
```

Если клиент отключился:

```
WebSocket умер
        │
Relay завершился
        │
Операция продолжает работать
```

Это автоматически устраняет проблему, описанную в §3.5.

---

# Шаг 4. Нативный Rust UI

Теперь тот же самый API можно использовать без HTTP вообще.

```rust
let (op, mut events) =
    OperationHandle::begin(None, stages);

task::spawn(async move {
    while let Ok(msg) =
        events.recv().await
    {
        app.update_progress_bar(msg);
    }
});

core.instance_service()
    .init(data, true, &op)
    .await?;
```

Никаких:

- HTTP
- Tide
- WebSocket

Только события.

GUI становится ещё одним подписчиком.

---

# Итоговая архитектура

```text
                  InstanceService
                         │
                         ▼
                 OperationHandle
                         │
                         ▼
               async-broadcast Channel
                         │
        ┌────────────────┼────────────────┐
        ▼                ▼                ▼
   WebSocket         Tasks Registry       GUI
```

Домен сообщает только **что произошло**.

Подписчики сами решают:

- отправить в WebSocket;
- обновить GUI;
- сохранить прогресс;
- зарегистрировать задачу;
- сделать что-либо ещё.

Именно это позволяет сделать библиотечную сборку без зависимости от Tide.

---

# Нюанс реализации

Использование

```rust
try_broadcast()
```

вместе с

```rust
set_overflow(true)
```

— осознанное решение.

Идея следующая:

- прогресс можно потерять;
- операцию нельзя тормозить.

Если слушатель не успевает получать обновления, новые сообщения просто вытесняют старые.

При этом финальный результат операции **не должен** передаваться через канал.

Он должен возвращаться обычным способом:

```rust
Result<T, E>
```

Канал используется только для потоковых обновлений прогресса.
