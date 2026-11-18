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
Новые GUI lifetime/reconciliation/diagnostic/recovery flows — следующие patches.
Это compile/test evidence, не ручной launch/install и не Windows/macOS PASS.
