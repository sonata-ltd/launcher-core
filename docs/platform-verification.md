# P11-V02 — Core platform verification (частичный результат)

Локальный checkpoint: **19 ноября 2026**. Этот шаг намеренно **не запускает и не
проверяет GUI**: пользователь отложил явную GUI verification. Исходный launcher
не переключён. P11-V01 GUI replay остаётся отдельным историческим evidence.

## Что проверено

| Область | Реальная проверка | Результат |
|---|---|---|
| Linux x86_64, core | fmt, check --all-targets, test, build; locked/offline | PASS: 572 library + 2 CLI + 2 integration + 18 doctests; 0 failed, 1 ignored |
| Windows GNU из Linux | fmt, check --all-targets, test --no-run, build; locked/offline | Cross-compile/link PASS; **Windows execution не выполнялся** |
| Windows MSVC из Linux | fmt, check --all-targets | fmt PASS; check FAIL в C dependencies: GNU compiler не поддерживает MSVC target; test-compile/build not-run |
| macOS | Inventory установленного toolchain | Target/native runner отсутствует; compile/runtime не проверены |
| Python verification runner | 8 stdlib unittest contracts | PASS; mock tests не являются platform execution evidence |
| GitHub Actions matrix | YAML/static contract checks | Настроен Linux/Windows/macOS native CI; hosted jobs **не запускались** |

Здесь native означает ОС, на которой выполняется **core test binary**, не
native GUI interaction. Windows GNU linking включает CLI и Rust test executables;
`test --no-run` не запускает assertions и не заменяет Windows filesystem/process
проверку. Установленный Windows target сам по себе не доказывает выполнение.

Исходная точка — core HEAD `083d7cc` плюс test-only изменения этого шага.
Production Rust code, migrations, dependencies и GUI patches 01–05 не менялись.
Runtime timestamp в raw reports сохранён как есть (UTC по часам окружения), без
переписывания под локальную дату checkpoint.

## Переносимые process regressions

`src/instance/running.rs` больше не отключает на Windows следующие четыре tests:

- `shutdown_waits_for_admitted_preparation_and_confirmed_process_exit`: admission
  закрыт при shutdown, уже допущенная preparation передаёт child без readmission;
  shutdown ждёт подтверждённого выхода.
- `the_watcher_reports_the_exit_code_and_clears_the_entry`: controlled child
  заканчивается с code 3, watcher публикует terminal event и снимает reservation.
- `native_directory_survives_running_process_and_is_removed_before_exit_event`:
  scratch и process pin защищены до выхода, cleanup завершается до Exited event.
- `kill_ends_the_process`: explicit kill завершает child; Unix signal exit и
  Windows termination code проверяются раздельно, повторный kill — NotRunning.

Вместо `sh`/`sleep` tests запускают `utils::process::tests::controlled_child_fixture`
в отдельном экземпляре **текущего Rust test binary**. Ready/exit handshake находится
в TempDir, ожидания ограничены. Если parent удалил TempDir или не управляет child
30 секунд, fixture выходит, не оставаясь постоянным orphan. Это реальный дочерний
процесс, но **не Java, game, installer или detached descendant**.

Все шесть `instance::running::tests` реально прошли на Linux; четыре указанных
tests скомпилированы/слинкованы для Windows GNU. Рост library totals 571 → 572
включает entry point subprocess fixture, не новый production feature или доказанную
Windows runtime-проверку. Windows/macOS assertions ещё должны выполниться на их ОС.

## Воспроизведение

Из core repository на native Linux/Windows/macOS runner с Rust/C compiler/Python:

```sh
python docs/verify-platform.py --output /path/to/new-evidence-directory
```

Локальные offline/cross команды этого запуска:

```sh
python3 -m unittest discover -s docs -p test_verify_platform.py
python3 docs/verify-platform.py --offline --output /tmp/opencode/core-p11-v02-native
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-gnu \
  --output /tmp/opencode/core-p11-v02-windows-gnu
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-msvc \
  --output /tmp/opencode/core-p11-v02-windows-msvc
```

Указанные directories уже содержат evidence; для повтора нужен **новый** путь.
Runner отказывает при существующем output, не перезаписывая предыдущий report.
Он запускает Cargo вне repository/GUI config hierarchy, передаёт manifest явно,
включает SQLx offline metadata и `--locked`, не использует live/user DB URL или
CurseForge credential. `--offline` дополнительно запрещает скачивание зависимостей;
без него clean CI может загружать Cargo dependencies. Host Cargo config/compiler
environment остаются частью inputs, это не hermetic sandbox.

Native mode выполняет `cargo test` с doctests. Любой `--target` включает только
compile mode: `cargo test --no-run`, никогда target execution. Каждый command пишет
свой log; первый failure/error/timeout завершает pipeline, остальные checks имеют
`not-run`, не PASS. Report хранит scope/host/toolchain/commands/exit statuses,
core HEAD/status и lock SHA-256 до/после. Изменившийся lock делает итог FAIL;
runner не откатывает и не коммитит lock. Command timeout — failure, не graceful
launcher shutdown (launcher здесь не запускается).

8 runner tests проверяют native/cross command boundary, credential/DB/fixture env
exclusion, isolated cwd, failure/not-run, timeout, missing compiler, lock mutation
без revert и отказ overwrite evidence. Первый mock run выявил, что
`platform.platform()` тоже использует subprocess: platform metadata теперь явно
mocked в tests, реальный runner по-прежнему читает настоящую platform metadata.

## Native CI

`.github/workflows/core-platform.yml` задаёт independent native jobs на
`ubuntu-latest`, `windows-latest`, `macos-latest`; target/architecture берутся из
самого runner и записываются через `rustc -vV`, не угадываются по имени image.

- Native fmt/check --all-targets/test/build и Python runner regressions.
- Fail-fast отключён, failures не скрываются через continue-on-error.
- Checkout без persisted credential, token permissions — только contents:read.
- Logs и report загружаются через always() artifact step, даже после fail.
- Никакого GUI baseline, dependency switch или GUI запуска.

Workflow доступен для push/PR/manual dispatch после публикации ветки. Это
**конфигурация будущей проверки**, а не выполненный CI PASS. Push/dispatch в этой
сессии не выполнялись. Все actual Windows/macOS results остаются открытыми до
сохранённых native reports; runner/image/C dependency failures также нужно учитывать.

## Артефакты и сохранность

- `/tmp/opencode/core-p11-v02-native/{fmt,check,test,build}.log` и `report.json`.
- `/tmp/opencode/core-p11-v02-windows-gnu/{fmt,check,test-compile,build}.log`
  и `report.json`.
- `/tmp/opencode/core-p11-v02-windows-msvc/{fmt,check}.log` и `report.json`.
- `/tmp/opencode/core-p11-v02-runner-tests.log`.
- `/tmp/opencode/core-p11-v02-baseline.json` и `core-p11-v02-preservation.json`:
  source GUI inventory/status, core lock и source patch hashes.

Raw reports/logs находятся локально в `/tmp/opencode`, не являются durable
committed archive. Исходные 173 GUI files/Git status, patches 01–05 и local core
lock сохранены. Lock SHA-256:
`45a050b11dd430a83528d17c98f474c26f767a039eebec35dac61df3951cf2a3`.
`Cargo.lock` и ранее существовавший `not-a-database-url` не входят в этот commit.

## Что остаётся открытым

P11-V02 **частичен**, не завершён: actual native Windows/macOS reports; manual
isolated launch/install/content/recovery/normal close; реальные JVM/descendants,
live providers и external concurrent FS/DB replacement. GUI проверка отложена
пользователем, не отменена из критериев. Core ignore `probes_java_on_path` не PASS.
Unix-only symlink/shell/crash fixtures в других модулях этим шагом не объявлены
проверенными на Windows. Power-loss/OS/disk crash durability не обещается.
B02/B06/D08/S05/S06 сохраняют прежние partial ownership/provenance ограничения.
