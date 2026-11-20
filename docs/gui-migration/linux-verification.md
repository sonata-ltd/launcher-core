# P11-V01 — Итоговая Linux verification

Дата: **19 ноября 2026**. Проверенный core code: `9594de6`
(`fix(gui): expose content recovery with confirmed file-preserving abandon`).
V01 обновляет документацию, не меняет проверенный Rust code или numbered patches.

## Результат

| Область | Проверки | Результат |
|---|---|---|
| Core | fmt/check/test/build, locked/offline | PASS: 571 library + 2 CLI + 2 integration + 18 doctests; 0 failed, 1 ignored |
| Fresh GUI snapshot | patches 01–05 apply-check/apply, dependency resolve, fmt/check/test/build | PASS: 238 passed, 0 failed, 7 ignored |
| Patch boundary | `git apply --numstat` для каждого patch | Только Rust sources в `src/` и `examples/`; нет Cargo.toml/Cargo.lock/config или baseline archive |
| Исходный GUI | SHA-256 inventory и Git status до/после replay | 173 tracked/nonignored untracked файлов неизменны; status неизменён |
| Локальный core Cargo.lock | SHA-256 до/после проверок | Неизменён; не включён в commits |
| Refresh 20.11 (core `4f4f3b2`) | тот же полный replay без `--checks`/`--target-dir` | PASS: те же numbers, детали ниже |

Это **Linux compile/test/replay PASS**, не blanket P08/P11 или manual/platform PASS.
Последующий [P11-V02 core шаг](../platform-verification.md) добавляет platform runner,
CI configuration и Windows GNU compile/link evidence без GUI запуска/проверки.
Числа и inputs этого V01 checkpoint ниже сохранены, не переименованы в новый GUI PASS.
Повтор использует fresh source snapshot и общий external build cache, не запуск
исходного GUI. Все test suites повторно выполнялись, не только compile checks.

## Refresh от 20 ноября 2026

Refresh выполняет тот же полный fresh replay против текущего core code `4f4f3b2`
(`fix(content): resolve Windows physical file identity for legacy hardlink
migration`). Inputs не менялись: patches 01–05 с хешами ниже, GUI baseline
`5124dd2e` плюс пользовательское working tree (88 dirty entries), external config
`7ac1c3f72d8d1764a0f1ef06fefb4a0ccee0a7501f4d173e5f149f57e4dba6c`, Luminate
`2b25b508676272fbfab7034ec437067b99a54d40` observed clean. Запуск без `--checks`
и без `--target-dir`: новая копия, apply-check/apply каждого patch по порядку,
dependency switch и offline resolve только в копии, fmt/check/test/build
`--locked --offline`.

- Snapshot: `/tmp/opencode/core-gui-verify.kata3695`; все пять шагов PASS, полный
  Cargo output в его `verification-{resolve,fmt,check,test,build}.log`.
- GUI результат не изменился: **238 passed, 0 failed, 7 ignored**.
- Snapshot lockfile изменился по design копии:
  `7e478a271c61c90743aa866f109e40e997444d27c6601939c122b6e5985f33d1`. Local core
  lock и исходный GUI не менялись: тот же HEAD/working tree после replay.
- Baseline inventory на момент refresh: 175 tracked/nonignored файлов вместо
  записанных 173 — пользовательское дерево добавило два файла, patches 01–05
  применились к копии без conflict. Это новый baseline: старый PASS не переносится
  автоматически, refresh подтверждает его повтором с теми же inputs.

Core pipeline на этом core code: **620 library + 2 CLI + 2 integration + 18
doctests, 0 failed, 1 ignored** — записано в [P11-V02](../platform-verification.md)
для commit `4f4f3b2`; refresh не пере-запускал core pipeline, только GUI replay.
Raw evidence checkpoint (`core-gui-verify.gryce229`, `core-p11-final-*`) к моменту
refresh уже не существовало; numbers восстановлены повтором execution, не
сохранёнными логами. Snapshot этого refresh подчиняется тому же правилу
non-durable evidence.

## Входы и среда

- Linux x86_64, kernel `7.3.4-arch1-2`, glibc `2.44`.
- `rustc 1.98.1 (48a229cea 2026-09-01)`.
- `cargo 1.98.1 (797e8a9bc 2026-08-05)`.
- Исходный GUI: `/home/quartix/Projects/launcher-native/gui`, HEAD
  `5124dd2e56d8d72db42960834b0e8e4924ea4a70` **плюс пользовательское working tree**.
  Чистый checkout этого HEAD не равен проверенному baseline; полное пользовательское
  дерево не опубликовано. Runner копирует существующие tracked/untracked files,
  исключая ignored build/runtime data.
- Рабочий migrated GUI fixture: `/tmp/opencode/core-p11-gui`, snapshot commit
  `bca7961`; manifest/lockfile overrides остаются локальными, вне source commits.
- External Cargo config: `/home/quartix/Projects/launcher-native/.cargo/config.toml`;
  Luminate overrides из `/home/quartix/Projects/luminate`, HEAD
  `2b25b508676272fbfab7034ec437067b99a54d40`, observed clean status. Config не менялся.
- Core команды выполняются `docs/check.sh` вне GUI config hierarchy, SQLx offline.
  Сохранённый local core lock отличается от committed lock только `patch.unused`
  entries; dependency versions не изменялись этим verification commit.

## Воспроизведение

Из core repository:

```sh
bash docs/check.sh fmt -- --check
bash docs/check.sh check
bash docs/check.sh test
bash docs/check.sh build

python3 docs/gui-migration/verify.py \
  /home/quartix/Projects/launcher-native/gui \
  --config /home/quartix/Projects/launcher-native/.cargo/config.toml \
  --target-dir /tmp/opencode/core-p11-gui/target
```

**Без `--checks`** runner выполняет полный fmt/check/test/build. Он сначала
создаёт новую source copy, применяет patches по порядку с `git apply --check`,
затем меняет dependency и разрешает lock **только в копии**. Последующие команды
используют `--locked --offline`. При другом source baseline или external overrides
нужен новый replay; старый PASS не переносится автоматически.

## Артефакты этого запуска

- Core logs: `/tmp/opencode/core-p11-final-{fmt,check,test,build}.log`.
- Replay summary: `/tmp/opencode/core-p11-final-replay.log`.
- Fresh snapshot: `/tmp/opencode/core-gui-verify.gryce229`.
  Его `verification-{resolve,fmt,check,test,build}.log` содержат полный Cargo output.
- Preservation baseline: `/tmp/opencode/core-p11-verification-baseline.json`.
- Machine-readable итог: `/tmp/opencode/core-p11-final-evidence.json`.
- Артефакты перечисленных выше путей были очищены до refresh 20.11; они остаются
  записью результата, не доступным archive. Snapshot refresh:
  `/tmp/opencode/core-gui-verify.kata3695` с полным Cargo output тех же пяти шагов.

Raw logs/snapshot находятся локально в `/tmp/opencode`, не committed и не durable
архив. Данный файл фиксирует проверенные results/inputs/границы; при очистке temp
потребуется повтор команды, а не утверждение наличия прежних raw logs.

Core test summaries (checkpoint `9594de6`; core pipeline на HEAD `4f4f3b2` — 620
library + 2 CLI + 2 integration + 18 doctests, 0 failed, 1 ignored):

```text
library:        571 passed; 0 failed; 1 ignored
CLI parser:       2 passed; 0 failed
CLI subprocess:   1 passed; 0 failed
public consumer:  1 passed; 0 failed
rustdoc:         18 passed; 0 failed
GUI fresh replay: 238 passed; 0 failed; 7 ignored (refresh 20.11 подтвердил)
```

SHA-256 source patches:

| Patch | SHA-256 |
|---|---|
| 01-service-api.patch | `63eeb2a1c772e2293b42dedfab756c8b182e0ed863e29111c6d98c8913c57b31` |
| 02-host-lifecycle.patch | `f4988d8e049654b4c4762ecdf8670cdd9788c0fb6c2a7f6e3c5460edbeabcf9d` |
| 03-operation-reconciliation.patch | `ecec90e4bcf6561e10f883347a0bca068d7b06b323c595e144269c3f55054012` |
| 04-managed-reports.patch | `9d36aec93e1c004c199f557c0e0da83013fb807747bc8cbe6c399fc3e23bf027` |
| 05-content-recovery.patch | `8a4477d86747dd2618c9a337c35d0c2be391a2da39e48dfa1cd77c00383642a1` |

Preserved core lock SHA-256:
`45a050b11dd430a83528d17c98f474c26f767a039eebec35dac61df3951cf2a3`.
Inventory comparison подтверждает сохранение исходных inputs именно на интервале
этого final verification; не реконструирует неизвестные прошлые изменения.

## Проверяемые контракты

- External service consumer и 18 independent negative API contracts исключают
  raw SQL/registry/install/recovery/process capabilities и removed shims.
- CLI help/syntax subprocess fixtures не создают root/DB/recovery side effects.
- Core operation tests: subject до Start/history/serde, legacy uncorrelated,
  interrupted stage terminal, Finish после terminal commit, bounded completion TTL.
- GUI observer tests: subscribe-first/overflow, foreign domains/baseline exclusion,
  active/terminal без caller, stale callbacks/progress, cache/output preservation,
  pruned result unavailable и unknown timing.
- Managed reports: partial warnings, diagnostic-only equality, сохранение previous
  diagnostics после failed refresh, unknown code inspect и typed sources.
- Content recovery: реальные local SQLite/files conflict/retry/owner/leases tests,
  `abandoned_caller_keeps_abandon_commit_owned_and_shutdown_waits`; GUI confirmation,
  busy/stale/duplicate callbacks, retained failed-action diagnostic/source,
  public adapter/files/shutdown и reachable disabled-catalog page.

Page/headless tests используют controlled callbacks, не доказывают, что человек
прошёл native flow. Реальные journals создаются в core tests; GUI не получает raw SQL.

## Открытые критерии

**P11-V02 остаётся открытым.** Не проверены:

- Windows/macOS CI и runtime filesystem/process semantics.
- Manual isolated native launch/install/content/recovery/normal-close с живой игрой.
- Live provider API, реальные game/installer JVM и detached descendants.
- Concurrent external FS/DB replacement, power loss/OS crash/disk loss.
- Abnormal renderer/process termination как graceful shutdown; News lifetime вне Tasks.

Core ignore — `java::utils::tests::probes_java_on_path`. GUI ignores: один manual
news preview, три Wayland/X11 activation/shadow fixtures и три explicit performance
measurements. Они **не PASS**. Existing dead-code/unreachable/future-compatibility
warnings сохраняются; warning-free build и native presentation не заявлены.

B02/B06/D08/S05/S06 остаются частичными: historical ownership/hardlinks, missing
dependency metadata, catalogue relocation и aggressive SHA-1/virtual placement GC
не восстанавливаются guessing. API narrowing не защищает arbitrary external writers.
Power-loss durability не обещается. Исходный launcher не переключён; push не выполнялся.
