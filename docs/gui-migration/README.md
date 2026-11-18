# Изолированная миграция реального GUI

Исходный GUI не переключён и не изменён. Нумерованные source-only patches
применяются **по порядку** к копии его текущего tracked/untracked дерева.
`Cargo.toml`, `Cargo.lock`, внешний Cargo config и пользовательский baseline
не входят в patches. Полное пользовательское дерево не публикуется.

```sh
python3 docs/gui-migration/verify.py /path/to/launcher-native/gui \
  --config /path/to/external/cargo-config.toml
```

Runner создаёт новую копию в `/tmp/opencode`, проверяет применимость каждого
patch перед записью, меняет core dependency **только в копии**, разрешает её
offline и выполняет fmt/check/test/build. Последующие cargo commands используют
`--locked --offline`; все логи и изменённый snapshot lockfile остаются в копии.
Опциональные `--target-dir` и `--checks fmt check` позволяют переиспользовать
сборочный кеш или выполнить ограниченную проверку; это не полный PASS.
При отличающемся baseline `git apply --check` может отказать: исходное дерево
не меняется, ручной merge нужен только в новой копии. Внешние Luminate overrides
передаются аргументом, runner их не создаёт и не меняет.

## 01 — Закрытый service API

`01-service-api.patch`: observer imports `operations::message`, managed Java
stage labels, fixtures без raw Database/state и с валидными Prism SHA-256.
CLI переведён на root GlobalState и Cleanroom catalog facade в самом core.
Positive consumer — `tests/public_api.rs`; 18 rustdoc compile-fail contracts
проверяют отсутствие SQL/registry/install/recovery/process capabilities и shims.
Error DTOs остаются nameable через `instance::error`, DbError — через root export.

Ни snapshots, ни пути, ни history не дают retention pin. Arbitrary external
filesystem/SQL writers не поддерживаются и не становятся безопасными от
закрытия Rust exports. Managed registrar port допускает owned registration
adapter, но не raw доступ к launcher registry. Read-only observer API не обещает
replay processor output: ring хранит lifecycle/status, а не полный лог.

## Evidence и ограничения

Linux, реальный GUI snapshot: fmt/check/test/build offline, **220 passed,
0 failed, 7 ignored**. Core: **566 library + 2 CLI + 2 integration + 18 doctests
passed, 0 failed, 1 ignored**. Логи первой интеграции:
`/tmp/opencode/core-p11-{check,test,build}.log` и
`/tmp/opencode/core-p11-gui-{check,test,build}.log`.

Первый GUI test run: 214 passed, 6 failed из-за старых невалидных SHA-256 fixtures;
fixtures исправлены, повтор прошёл без ослабления parser.
## 02 — Host credentials и cooperative close

`02-host-lifecycle.patch`: credentials из host env передаются явно в Config;
blank credential не становится ключом. Main window перехватывает close request,
оставляет waiting view и ждёт `core.shutdown()` без abort/timeout. Повторные close
requests идемпотентны, routing/window creation на время close выключены.
Игры не убиваются автоматически: есть явный `Stop games`; shutdown иначе ждёт exit.
Три regressions проверяют config injection без env mutation, one-shot close state
и shutdown admission isolated core. Linux GUI fmt/check/test/build:
**223 passed, 0 failed, 7 ignored**, логи `/tmp/opencode/core-p11-lifecycle-*.log`.
Это normal-close wiring; аварийное завершение renderer/process не превращается
в graceful shutdown. News имеет собственный GUI lifetime и не объявляется частью
core Tasks. Native close с настоящей игрой вручную пока не проверен.

## 03 — Typed correlation и reconciliation

`03-operation-reconciliation.patch`: core snapshots несут `OperationSubject`,
заданный до Start. Names не используются для domain policy. Instance-install
slot не принимает content/managed/чужой instance job и исключает operations,
наблюдавшиеся до GUI begin. Generation token не даёт старому callback закрыть
новый run. Подписка создаётся до initial reconciliation signal; `recv()` явно
сообщает overflow. App читает актуальный `operations()` при обработке сигнала
и lifecycle/progress событий, а не применяет устаревший queued snapshot.

Active/completed восстанавливаются без caller; pruned terminal показывается как
неизвестный result, не success. Stage cache identities и доступный processor
output сохраняются. Lost output и длительности не выдумываются: восстановленный
completed stage показывает `Done (timing unavailable)`. Failed/cancelled running
stage имеет terminal status в core snapshot. Это один install presentation slot,
не dashboard всех одновременных jobs. Raw snapshots/history не retention pins.

Linux core fmt/check/test/build: **569 library + 2 CLI + 2 integration + 18
doctests passed**, 1 ignored. GUI: **229 passed**, 7 ignored. Логи:
`/tmp/opencode/core-p11-operations-{check,test,build}.log` и
`/tmp/opencode/core-p11-operations-gui-{check,test,build}.log`.
Fresh replay fmt/check проверен; native/manual/platform evidence отсутствует.

Diagnostic/recovery flows — следующие patches.
Это compile/test evidence, не ручной launch/install и не Windows/macOS PASS.
