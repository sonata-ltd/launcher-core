# Контракт потребителя ядра

## Service boundary и composition

`GlobalState`/`AppError` экспортируются из root, composition принадлежит `app`.
Потребитель вызывает services через `instances()`, `content()`, `java()`,
`options()`, `versions()` и `modloaders()`. Raw Database/pool, mutable registries,
low-level install/recovery/storage/process helpers и compatibility shims не входят
в public API. Error DTOs остаются nameable; DbError доступен через root export.

External `tests/public_api.rs` использует isolated root/DB и service API.
18 независимых `compile_fail` contracts в `src/lib.rs` проверяют отсутствие
инфраструктурных обходов. Это Rust API boundary, не sandbox для внешних FS/SQL writers.
DTO paths, snapshots, history и clones никогда не заменяют admission или process pin.

Host передаёт credential через `Config::curseforge_api_key`; core не читает credential
из env. `discover_java_on_startup = false` отключает только startup discovery, не
меняет persisted done flag и не запрещает explicit rediscovery. Root overrides не
требуют default home, actual instances resource имеет lifetime lock. Для изолированных
tests нужны временные root/DB, а не пользовательские launcher data.

## Observer identity и reconciliation

- Public DTOs находятся в `operations`; `GlobalState::operations()` атомарно
  возвращает active snapshots и bounded completed history. Default history —
  128 results/1 час, TTL от completion; pruning не запрещает новые jobs.
- `JobId = OperationId` — session identity. Content/managed installation UUIDs
  остаются durable domain identities и не получают session ID при disk recovery.
- `OperationSubject` задаётся до Start и сохраняется в snapshot/history:
  InstanceInstall, ContentInstall, ManagedInstall, ManagedValidation. Display name
  не является protocol. Legacy snapshot без subject остаётся uncorrelated;
  Start DTO содержит stages, не domain identity.
- Подписаться на bus нужно **до** initial reconciliation; после overflow снова
  читать fresh view. Finish публикуется после удаления active и сохранения terminal
  snapshot/outcome. Queued progress не должен побеждать authoritative snapshot.
- История не durable journal и не полный output log. Нельзя восстанавливать потерянные
  processor lines/durations по snapshot или объявлять pruned result успешным.

GUI patch 03 реализует один instance-install presentation slot, не dashboard всех
jobs. Baseline exclusion и RunGeneration защищают от чужой/старой операции и late
caller callback. Active/completed восстанавливаются без caller; доступные cache IDs,
output и timing сохраняются, lost timing показывается как unavailable.

## Typed errors и managed reports

Runtime error source удерживается отдельно от сериализуемой диагностики. Managed
codes используют `ManagedJavaErrorKind`, unknown code round-trips без guessed policy.
Retryability определяется typed transport/status/IO cause, не message/code substring.
После disk recovery diagnostic fields доступны, но runtime source не фабрикуется.

Потребитель читает `catalog_report()` и `installations_report()`, если должен показать
partial warnings и record-local diagnostics. Пустой healthy список не означает
отсутствие corrupt records; failure следующего refresh не стирает прежнюю диагностику.
Diagnostic-only изменения должны участвовать в polling equality. Bad records остаются
на месте и продолжают блокировать GC.

GUI patch 04 удерживает UiError kind/diagnostic chain/Arc typed source в Java
runtime/settings flow. Unknown managed codes получают Details/Inspect, не guessed
Validate/Reinstall. Остальные исторические string adapters не объявлены полностью
мигрированными. Patch 05 типизирует именно content recovery.

## Recovery и cooperative shutdown

Экспортируемые DB-backed сервисы удерживают lifetime locks до освобождения
последнего владельца. Остановка закрывает admission для новых изменений
экземпляров, настроек и регистрации Java. Чтение доступно для уже
инициализированных сервисов. Допущенные процессы и recovery завершаются
с удержанием их ресурсов.

SQLite запоминает layout при первом открытии обновлённым ядром. Открытие
этой же базы с другой конфигурацией ресурсов возвращает `DbError::LayoutConflict`.
Перед первым открытием исторической базы требуется корректная конфигурация
её каталогов. Для переноса уже привязанного layout нужен отдельный процесс миграции.

Content recovery использует `pending_recovery`, `retry_recovery`, `abandon_recovery`.
Pending gate действует и без active job. Retry предполагает сохранение/перемещение
конфликтующего файла пользователем; abandon выбранного journal/leases не изменяет
файлы и отвергает чужой instance/unknown ID. Уже допущенный abandon SQL commit
принадлежит Tasks worker с activity guard, переживает caller drop; shutdown его ждёт.
Подробности: [publication contract](publication-contract.md).

GUI patch 05 предоставляет recovery при disabled Mods catalog, без automatic network
search. Abandon требует confirmation. Busy/single-flight/generation gates запрещают
двойной action и stale callback; successful polling не стирает failed action diagnostic.

`core.shutdown()` закрывает Tasks admission, сигнализирует cooperative cancellation
и ждёт publishing/SQL/blocking cleanup без abort. Игры не убиваются автоматически:
shutdown может ждать их exit. GUI normal close оставляет waiting view; Stop games
доступен только как explicit action пользователя. News имеет отдельный GUI lifetime,
не часть core Tasks. Renderer/process crash не обещает graceful shutdown.

CLI pure parsing/help выполняются до dotenv/config/bootstrap; help/no args — exit 0,
syntax error — exit 2 без root/DB/recovery side effects. Runtime dispatch errors проходят
shutdown. Syntax validation не обещает успешного domain/network выполнения.

## Проверка и переключение

[Numbered GUI patches](gui-migration/README.md) применяются только к копии реального
tracked/untracked GUI дерева. Dependency/lockfile меняются только в копии; source
patches не включают пользовательский baseline. Исходный launcher автоматически не
переключается. Перенос в исходный GUI требует отдельного проверенного изменения.

[Linux verification](gui-migration/linux-verification.md) покрывает fmt/check/test/build,
service/API/reducer/headless regressions и full fresh replay, не Windows/macOS,
manual native launch/install/content, реальные JVM/descendants или power-loss.

[P11-V02 core platform runner](platform-verification.md) добавляет native CI matrix
и Linux/Windows GNU compile/link evidence. Hosted Windows/macOS jobs ещё не запускались;
cross test --no-run не выполняет assertions. Четыре lifecycle regressions используют
controlled Rust child без shell/Java. Явная GUI verification отложена пользователем;
этот шаг не меняет GUI и не закрывает manual/native/platform критерии.
