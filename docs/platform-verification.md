# P11-V02 — Core platform verification (частичный результат)

Локальный checkpoint: **19 ноября 2026**. Этот шаг намеренно **не запускает и не
проверяет GUI**: пользователь отложил явную GUI verification. Исходный launcher
не переключён. P11-V01 GUI replay остаётся отдельным историческим evidence;
его refresh от 20 ноября против этого commit выполнен с теми же inputs и
подтвердил прежние numbers ([gui-migration/linux-verification.md](gui-migration/linux-verification.md)).

## Что проверено

| Область | Реальная проверка | Результат |
|---|---|---|
| Linux x86_64, core | fmt, check --all-targets, test, build; locked/offline | PASS: 572 library + 2 CLI + 2 integration + 18 doctests; 0 failed, 1 ignored |
| Windows GNU из Linux | fmt, check --all-targets, test --no-run, build; locked/offline | Cross-compile/link PASS; **Windows execution не выполнялся** |
| Windows MSVC из Linux | fmt, check --all-targets | fmt PASS; check FAIL в C dependencies: GNU compiler не поддерживает MSVC target; test-compile/build not-run |
| macOS | Inventory установленного toolchain | Target/native runner отсутствует; compile/runtime не проверены |
| Python verification runner | 8 stdlib unittest contracts | PASS; mock tests не являются platform execution evidence |
| GitHub Actions matrix | YAML/static contract checks | Настроен Linux/Windows/macOS native CI; hosted jobs **не запускались** |
| Linux x86_64, core — этот commit (`fix(content): resolve Windows physical file identity…`) | fmt, check --all-targets, test, build; locked/offline | PASS: 620 library + 2 CLI + 2 integration + 18 doctests; 0 failed, 1 ignored; lock_unchanged true |
| Windows GNU из Linux — этот commit | fmt, check --all-targets, test --no-run, build; locked/offline | Cross-compile/link PASS; Windows execution не выполнялся; lock_unchanged true |
| Windows native — dual-boot пользователь | Command из раздела «Воспроизведение», выполняемый на Windows | **Pending**: выполнит пользователь; commit land'ится с native Windows pending |
| Real JVM detection (real JDK) | отдельный run `--include-ignored`, документирован ниже | PASS от 20 ноября: `probes_java_on_path` 1 passed, real JDK `27.0.1.0.3` |
| Live providers — endpoint observations от 20 ноября | точные code-path URLs через curl, без credentials/user DB | Modrinth `/v2/search` → HTTP 200 real JSON; CurseForge `/v1/mods/search` без `x-api-key` → HTTP 403 «API Key missing or invalid» |
| Live providers — кодовый путь от 20 ноября (этот commit) | 5 execution реального кода против live API из внешнего scratch-потребителя через public API | Modrinth search/dispatch/normalize/exact_release PASS; CurseForge: fail-closed `MissingCurseForgeKey` до сети; execution с invalid key → `Err Http(403)` |

Здесь native означает ОС, на которой выполняется **core test binary**, не
native GUI interaction. Windows GNU linking включает CLI и Rust test executables;
`test --no-run` не запускает assertions и не заменяет Windows filesystem/process
проверку. Установленный Windows target сам по себе не доказывает выполнение.

Исходная точка — core HEAD `083d7cc` плюс test-only изменения этого шага.
Production Rust code, migrations, dependencies и GUI patches 01–05 не менялись.
Runtime timestamp в raw reports сохранён как есть (UTC по часам окружения), без
переписывания под локальную дату checkpoint.

Этот commit (`fix(content): resolve Windows physical file identity for legacy
hardlink migration`) стоит поверх `b20fe94` и меняет production code: path-based
`same_physical_file` (`src/instance/content/store/legacy.rs`), одна windows-sys
feature (`Win32_Storage_FileSystem`) и test gates. Прежние строки таблицы относятся
к checkpoint `083d7cc`; строки этого commit'а добавлены рядом и подтверждают
`lock_unchanged: true`. GUI patches 01–05 не менялись, GUI не запускался.

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

Локальные offline/cross команды этого запуска и этого commit'а:

```sh
python3 -m unittest discover -s docs -p test_verify_platform.py
python3 docs/verify-platform.py --offline --output /tmp/opencode/core-p11-v02-native
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-gnu \
  --output /tmp/opencode/core-p11-v02-windows-gnu
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-msvc \
  --output /tmp/opencode/core-p11-v02-windows-msvc
# этот commit (новые dirs, lock_unchanged true):
python3 docs/verify-platform.py --offline --output /tmp/opencode/core-b02-native-2
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-gnu \
  --output /tmp/opencode/core-b02-windows-gnu-2
# native Windows evidence — выполнить на dual-boot Windows (native mode, не cross):
python docs/verify-platform.py --output /tmp/opencode/core-b02-native-windows
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
- `/tmp/opencode/core-b02-native-2/{fmt,check,test,build}.log` и `report.json` —
  этот commit, offline native Linux, `lock_unchanged: true`.
- `/tmp/opencode/core-b02-windows-gnu-2/{fmt,check,test-compile,build}.log` и
  `report.json` — этот commit, Windows GNU cross, `lock_unchanged: true`.
- `/tmp/opencode/core-b02-native-windows/` — pending: native Windows evidence,
  который выполнит пользователь на dual-boot.

Raw reports/logs находятся локально в `/tmp/opencode`, не являются durable
committed archive. Исходные 173 GUI files/Git status, patches 01–05 и local core
lock сохранены. Lock SHA-256:
`45a050b11dd430a83528d17c98f474c26f767a039eebec35dac61df3951cf2a3`.
`Cargo.lock` и ранее существовавший `not-a-database-url` не входят в этот commit.

## Real-JVM probe от 20 ноября

Единственный core ignore, `java::utils::tests::probes_java_on_path`, выполнен на
этой Linux-машине с real JDK `27.0.1.0.3`: отдельный run по documented env
`docs/check.sh` (SQLx offline, locked, без GUI config hierarchy):

```sh
bash docs/check.sh test -- --include-ignored java::utils::tests::probes_java_on_path
```

Результат: **1 passed, 0 failed** (`620 filtered out`; остальные targets — 0 tests).
`detect_properties("java")` вернул `Some` major, arch и home_path — контракт
детекта real JVM на PATH выполнен кодом core, не fixture. Полный стандартный
Linux pipeline этого commit сохраняет прежние numbers: `620 library + 2 CLI + 2
integration + 18 doctests, 0 failed, 1 ignored` — probe запускался отдельно,
standard run не пере-запускался этим шагом.

Это **только** real-JVM detection на PATH этой машины: live game/installer JVM,
detached descendants, retention/ownership поведения JVM-roots, Windows/macOS
JVM, manual GUI и live providers остаются открытыми. Log:
`/tmp/opencode/core-jvm-probe-20nov.log` — локальный non-durable evidence.

## Live-provider endpoint observations от 20 ноября

Сеть на этой машине доступна. curl проверяет **точно те URLs, которые строит код**
(`src/instance/content/catalog/{modrinth,curseforge}/mod.rs`), без credentials и
без user DB:

- `GET https://api.modrinth.com/v2/search?mod=sodium&facets=[["project_type:mod"]]&limit=3`
  → **HTTP 200**, real JSON hits (`project_id`/`project_type`/`slug` поля) —
  `/tmp/opencode/live-modrinth.json`.
- `GET https://api.curseforge.com/v1/mods/search?gameId=432&classId=6&searchFilter=sodium&gameVersion=1.21.1&modLoaderType=5&index=0&pageSize=20`
  без `x-api-key` → **HTTP 403** `Forbidden: API Key missing or invalid` —
  `/tmp/opencode/live-curseforge-nokey.json`. Совпадает с fail-closed контрактом:
  код требует `CURSEFORGE_API_KEY` (`MissingCurseForgeKey`) до запроса, и без ключа
  live path не выполняется; ключа на этой машине нет.

Это **HTTP-level reachability/auth наблюдение**, не execution кодового пути:
DTO/normalize/search поведение на реальных responses не проверено. Live providers
этим шагом не объявляются PASS; `CURSEFORGE_API_KEY` на этой машине отсутствует.
Raw JSON — локальный non-durable evidence.

## Live-provider кодовый путь от 20 ноября (этот commit)

Сеть на этой машине доступна. Реальный код core выполнен из **внешнего scratch-потребителя**
`/tmp/opencode/core-scenarios` (path-dep на core, lock scratch изменён, repo lock не
двигался) — только через public API: `ContentCatalog::new/search/releases/exact_release`,
`modrinth::Modrinth::new/search/releases/exact_release`, `instance::content::model::{Provider}`,
`catalog::types::Compatibility`, `utils::files::child`. Standard pipeline этого commit
не пере-запускался: `620 library + 2 CLI + 2 integration + 18 doctests, 0 failed, 1 ignored`
сохраняются; новых core-тестов этот шаг не добавляет (scratch binaries — не core tests).
Build: locked-режим `check.sh` env (SQLx offline, openssl-3, `--offline` на scratch).
Query/compatibility для наблюдений: `query=sodium`, `minecraft=1.21.1`, `loader=fabric`.

Результаты (OBS-лог `/tmp/opencode/core-wine-evidence/d_live_providers.log` — локальный
non-durable evidence):

1. `ContentCatalog::search(Modrinth, …)` → `Ok`: `total=49`, `returned=20`, top hit
   `AANobbMI|Sodium` — live dispatch по реального ответа.
2. `Modrinth::releases` на top hit → `Ok`: `count=12`, first `SMxNOGZ6|file=sodium-fabric-0.8.13+mc1.21.1.jar`
   `host=cdn.modrinth.com size=1574609 deps=0` — normalize по реальным responses; DTO-поля
   (file_name/size/url host) заполнены против live данных.
3. `ContentCatalog::exact_release(Modrinth, …)` на первый normalized release → результат
   идентичен direct-provider normalize path (`SMxNOGZ6`, тот же file/host/size).
4. Live code path CurseForge с intentionally invalid key → `Err Http(403)` — живой ответ
   API, контракт fail-closed по auth выполнен кодом.
5. Keyless call → `Err MissingCurseForgeKey` **in code** — error до network call,
   подтверждает contract observation предыдущего commit'а уже самим кодом.

Это **execution кодового пути против live responses** (live network calls, реальные DTO/normalize).
Closes the observation-only gap для Modrinth-ветки; CurseForge success path открыт до
`CURSEFORGE_API_KEY` (keyless fail-closed и invalid-key `Err` execution — закрыты этим
шагом). Live providers этим шагом не объявляются полностью закрытыми.

## B01-спостереження: конкурентна підміна предка (20 ноября, этот commit)

Scratch-потребитель `/tmp/opencode/core-scenarios` (path-dep на core, scratch lock
изменён, repo lock не двигался) выполнил против shipped кода `utils::files::child`
два сценария по 7 повторов каждый, только через public API:

- **Static:** 7/7 отсутствующих targets под escaping symlink отклонены обычным
  путём shipped `child()` — Regression-контракт (missing target, broken symlink)
  работает.
- **Race:** при конкурентной подмене предка 7/7 записей физически landed
  вне корня — documented window (коммент в `src/utils/files/mod.rs`: "This check
  assumes another process isn't replacing directories during I/O") подтверждён
  наблюдением. Исправление конкурентного пути не заявляется; статус B01 без
  изменений («Исправлен обычный путь»).

Стандартный Linux pipeline не пере-запускался: `620 library + 2 CLI + 2 integration
+ 18 doctests, 0 failed, 1 ignored` сохраняются; новых core-тестов этот шаг не
добавляет (scratch binaries — не core tests). Log:
`/tmp/opencode/core-wine-evidence/e_prefix_race.log` — локальный non-durable
evidence (`OBS static-prefix-rejected: 7/7`; `OBS race-bytes-landed-outside-root: 7/7`).

## Что остаётся открытым

P11-V02 **частичен**, не завершён: actual native Windows/macOS reports; manual
isolated launch/install/content/recovery/normal close; реальные JVM/descendants
(detected-properties probe — PASS выше; WINE-execution кодового пути — ниже; native
Windows execution остаётся authoritative), live providers (Modrinth code path выполнен
против live responses — см. секцию выше; CurseForge открыт до
`CURSEFORGE_API_KEY`) и external concurrent FS/DB replacement. GUI проверка отложена
пользователем, не отменена из критериев.
Unix-only symlink/shell/crash fixtures в других модулях этим шагом не объявлены
проверенными на Windows. Power-loss/OS/disk crash durability не обещается.
B02/B06/D08/S05/S06 сохраняют прежние partial ownership/provenance ограничения.
Windows physical identity в B02 реализован (path-based `GetFileInformationByHandleEx`/
`FILE_ID_INFO`, fail-closed `None`); единственное отсутствующее доказательство B02 —
native Windows execution от dual-boot пользователя (команда выше), остальное —
прежние fail-closed ограничения контракта.
