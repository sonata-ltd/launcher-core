# Рефакторинг ядра: контракты и проверка

Актуализировано: 19 ноября 2026. Основание: [архитектурный аудит](core-architecture-audit-2026-11-17.md).

## Рабочая копия и исходная точка

Рабочий каталог: [core-refactor](../Cargo.toml). Это самостоятельный Git-репозиторий на ветке `refactor/all-audit-findings`, созданный из снимка **тогдашнего рабочего дерева** `/home/quartix/Projects/launcher/core`, включая незакоммиченный контент и SQLx metadata. История оригинального репозитория не клонировалась; его HEAD был записан в историческом manifest. Снимок — первая локальная контрольная точка копии: `69ca8df619b4`.

Скопировано 227 файлов, SHA-256 каждого проверен. Все 178 Rust-файлов совпали с инвентарём аудита. Скопированы Cargo.toml/Cargo.lock, src, migrations, .sqlx, .cargo, README/LICENSE/.gitignore и local_docs. Не копировались runtime/build данные: target, dev.db, .env с credentials, logs, .crush и исходный .git. Для тестов использовать временные БД/каталоги; рабочие данные launcher не нужны.

- `baseline-manifest.json` и `source-status.txt`: исторические материалы снимка; в этой рабочей копии отсутствуют.
- [TASKS.md](TASKS.md): полный реестр замечаний с пакетами реализации.
- [publication-contract.md](publication-contract.md): порядок FS/SQL и явные границы process-crash гарантии; power-loss durability не обещается.
- [retention-contract.md](retention-contract.md): scratch cleanup, conservative published GC и обязательные roots/admission/process/age gates.
- [asset-index-retention-contract.md](asset-index-retention-contract.md): D08 follow-up — строгий typed schema private asset index roots, duplicate rejection и known-type validation.
- [consumer-contract.md](consumer-contract.md): service API, observer reconciliation, typed diagnostics, recovery и shutdown.
- [gui-migration/README.md](gui-migration/README.md): source-only GUI patches 01–05 и изолированный runner.
- [gui-migration/linux-verification.md](gui-migration/linux-verification.md): итоговые Linux evidence и непроверенные критерии.
- [platform-verification.md](platform-verification.md): P11-V02 core native/cross runner, CI matrix и непроверенные Windows/macOS/GUI критерии.
- [legacy-hardlink-migration.md](legacy-hardlink-migration.md): B02 physical-identity rename proof, owned migration/SQL lifetime и fail-closed ограничения.
- [check.sh](check.sh): воспроизводимый запуск проверок без GUI Cargo patches.
- `verify-baseline.py`: историческая проверка снимка; в этой рабочей копии отсутствует.

Реализация ведётся в `core-refactor`. GUI использует зависимость `../../launcher/core`. Текущий статус исправлений и проверки приведены в [реестре замечаний](TASKS.md).

## Сборочная среда

Для совместимости с openssl-sys 0.9.109 используются установленные на машине библиотеки и заголовки OpenSSL 3. `check.sh` запускает Cargo из временного каталога и изолирует копию от патчей зависимостей GUI. Исходный Cargo.lock сохранён.

```sh
export OPENSSL_INCLUDE_DIR=/usr/include/openssl-3
export OPENSSL_LIB_DIR=/usr/lib/openssl-3
bash docs/check.sh fmt -- --check
bash docs/check.sh check
bash docs/check.sh test
bash docs/check.sh build
```

Исторический baseline: 227 тестов прошли, 16 упёрлись в запрет локальных сокетов, один проигнорирован. Перед продолжением 18 ноября 2026 полный запуск в текущей среде: **284 passed, 0 failed, 1 ignored**, включая локальные HTTP-серверы. Это не проверка живых API и не Windows/macOS runtime.

## Правила выполнения

1. Одна задача — один Conventional Commit в копии, вместе с её tests/patch/evidence. Перед переносами модулей сначала зафиксировать поведение тестами. Не смешивать функциональное исправление, массовый rename и schema migration в одном коммите.
2. Каждый пункт TASKS должен получить проверку, реализацию и ссылку на commit/тест. Замечания аудита включают риски и рекомендации: сначала подтвердить актуальность, затем исправить дефект или явно определить и проверить контракт. Нельзя считать любой переезд файла устранением архитектурной проблемы.
3. Не редактировать применённые миграции 0001–0008. Новые constraints/форматы вводить следующими миграциями с upgrade fixtures. Journal версии 0 читать до перехода на версию 1; unknown schema возвращает диагностику конкретной операции.
4. До сужения pub API проверить потребителей GUI/CLI. A07 удалил compatibility API после inventory и проверки реального GUI в изолированной копии; numbered patches воспроизводят миграцию, исходный GUI не переключён. Не возвращать low-level exports ради обхода service-инвариантов.
5. Temporary paths и отдельная memory/file SQLite; no-clobber не заменять overwrite. После подготовки recovery intent отмена не должна оставлять неопределённый commit.
6. Для каждого пакета: fmt, релевантные regression tests и cargo check. На границах этапов: полный test/build; дополнительно Windows/macOS CI и ручная проверка UI для изменившихся контрактов. Blocked проверка никогда не считается PASS.

## Пакеты реализации и критерии завершения

### P00 — Проверяемая исходная точка

Снимок/manifest/локальная ветка, рабочая сборочная среда, baseline test/build. Сохранить категории существующих failures. Снимок готов, несовместимость устранена выбором системной OpenSSL 3. Проверка HTTP требует среды с доступными локальными сокетами.

### P01 — Безопасные файловые операции

Исправить missing/broken symlink и processor output containment, проверять наличие/целостность assets cache, валидировать digest перед slicing/path join, запретить нулевые параметры буферов. Default content placement — изолированная copy/reflink; atomic cache-hit copy. Чётко определить ограничения TOCTOU при внешних writers.

**Готовность:** записи не выходят за root; A не меняет CAS/B при default policy; no-clobber сохраняет чужой target; invalid Unicode/hash/path/zero input не вызывает panic; repair восстанавливает потерянные/повреждённые assets. Тесты Linux и Windows filesystem semantics.

### P02 — Admission, журнал и восстановление

Общий coordinator launch/install/content/delete и узкий pending gate; recovery по отдельным операциям/instances; versioned typed journal; conflict query и команды retry/keep-external/abort; записывать observed placement.

**Готовность:** pending SQL без in-memory job блокирует все конфликтующие сценарии; плохой JSON A не мешает B; unknown schema диагностируется; повтор восстановления идемпотентен; пользовательский конфликт разрешается без прямой правки БД; crash fixtures после каждого FS/SQL шага.

### P03 — Актуальный inventory и installed-aware план

Сквозной scan/watch/reconcile, presence/identity, adoption существующего файла, snapshot validation, repository вместо SQL в service. Resolver учитывает installed/enabled/pinned и unknown provenance.

**Готовность:** ручное удаление/переименование/замена отражаются в планах; adoption пишет provenance; установленный incompatible блокирует новый план; disabled/pinned имеют явную семантику; watcher error/overflow не выдаёт кеш за verified.

### P04 — Каталоги и подбор кандидатов

Candidate-local отказ не обрывает пригодные альтернативы; bounded request budget и кеш exact responses; одна compatibility policy для exact/list; partial managed catalog results с warnings по документированному контракту.

**Готовность:** плохой старый кандидат + пригодный новый разрешаются; точный CF-файл другого loader отвергается; unsupported relation не становится пустым success; сетевой отказ отличается от отсутствия совместимых версий.

### P05 — Единые мутации settings и согласованный cache

Typed effective memory/args validation; атомарный overview+rename; один владелец событий, query projections отдельно; cache invalidation/versioning; одинаковый mapping конфликтов имён.

**Готовность:** rejected rename не сохраняет tags; quoting пробелов сохраняется; inherited min/max валидируются; Options/Instance дают одинаковые domain events; concurrent rename/read не возвращает устаревший cache как новый факт.

### P06 — Общая инфраструктура транспорта/отмены/файлов

Один bounded bytes/JSON transport и streaming engine с configurable status/redirect/size/hash/retry policy. Opaque cancellation с awaitable signal. Общие copy+digest/tree helpers; один HostPlatform; URL-aware manifest cache; явный контракт utility APIs.

**Готовность:** все адаптеры проходят общую матрицу status/redirect/limit/cancel/retry; нет downgrade HTTPS в policy requiring HTTPS; 404/permission не повторяются как transient; cancel прерывает read/backoff/assets/verify; нейтральная инфраструктура не зависит от instance/modloader.

### P07 — Один launch pipeline и корректные runtime policy

JavaPolicy в java; разделить game/installer requirements; legacy data adapter в общий LaunchPlan/rules/args/natives. Подключить Java validation, убрать второй mods scanner/эвристические helpers после миграции. Владение natives связано с процессом.

**Готовность:** несовместимый major/arch отвергается до spawn; installer не выбирает Java только по max major; legacy/modern fixtures используют общие rules; cwd корректен; extraction errors не теряются; natives удаляются после exit.

### P08 — Object store, миграции и жизненный цикл данных

Общие bytes identity/aliases, отдельно roles/placements/provenance. Migration legacy libraries, relative paths, constraints association ownership. Leases с начала подготовки; GC old generations/CAS/staging; согласованный durable publication protocol.

**Готовность:** одинаковые bytes на разных путях/ролях не меняют другой экземпляр; upgrade сохраняет installed data/journals; GC не удаляет leased/current files; crash recovery и заявленная power-loss durability проверены на поддерживаемых платформах. Если обещается только process-crash гарантия — это явно отражается в API/документации, нельзя молча объявить power-loss риск исправленным.

### P09 — Задачи, history и shutdown

Общие JobId/cancel/terminal outcomes, domain-specific phases отдельно; managed validation через registrar port; bounded history с finished_at; managed recovery изолирует плохие records; task supervision и shutdown.

**Готовность:** terminal result доступен после переподписки; panic даёт failed вместо вечного active; history не запрещает новые jobs; повреждённый managed record не скрывает исправные; shutdown не отменяет durable publishing посередине.

### P10 — Composition root, ошибки и локальные контракты

App/bootstrap вне data; ContentInitError вместо всего ContentError в bootstrap; explicit config credentials/paths/lock identity; typed causes без UI localization. Исправить eager paths fallback, CLI help/init order, serialization Cleanroom registry, invalidation catalog semantics.

**Готовность:** infrastructure не собирает domain services; root overrides не требуют default home; конкурентные register не теряют entries; force invalidation не возвращает прежний in-flight/disk snapshot; CLI help не запускает recovery. Root lock соответствует фактической области общего ресурса.

### P11 — Закрытие API и интеграция

Сузить storage/registry/pool exports после consumer inventory. Удалить неподключённые java/parse и utils/system после проверки истории. Перевести реальные GUI/CLI импорты и убрать временные shims только после этого.

**Готовность:** GUI/CLI compile contracts, ручной запуск/установка/управление контентом в изолированном root, все TASKS закрыты доказательствами; полные fmt/check/test/build и platform CI. Исходный launcher переключается на результат только отдельным проверенным изменением.

Зависимости: P00 → P01/P02; P02 → P03 → P04; P05 и P06 можно выполнять после P01 отдельными последовательными пакетами; P06/P05 → P07; P02/P03/P06/P07 → P08; P02/P06 → P09; P09/P07 → P10; все пакеты → P11. Это зависимости изменений, не распоряжение запускать параллельных агентов.

## Текущее состояние

**B02 follow-up:** startup migration изолирует однозначно переименованные/disabled
CAS hardlinks по physical identity — Unix device/inode и Windows volume serial + 128-bit
file ID (`GetFileInformationByHandleEx`/`FILE_ID_INFO`, fail-closed `None`), не по equal
bytes; competing bindings/aliases и unknown ownership остаются отложенными. Owned Tasks
worker удерживает activity, CAS admission и lifetime locks после bootstrap drop до SQL
completion/shutdown. `remove_file`-перед-`persist` доказанно избыточен (Windows
REPLACE_EXISTING/REPLACE semantics), production `detach()` unchanged. 18 Linux
regressions PASS (path-based `same_physical_file` asserts, семь gates
`any(unix, windows)`); полный core fmt/check --all-targets/test/build:
**620 library + 2 CLI + 2 integration + 18 doctests**, 0 failed, 1 ignored. Windows
GNU cross — compile/link PASS без execution; native Windows execution pending
(dual-boot пользователь). GUI не запускался и не проверялся.
[Evidence и оставшиеся partial cases](legacy-hardlink-migration.md).

**P11-V02 (core часть):** native CI matrix Linux/Windows/macOS настроен, но hosted
jobs ещё не запускались. Linux core fmt/all-target check/test/build: **572 library
+ 2 CLI + 2 integration + 18 doctests passed**, 1 ignored. Windows GNU all-target
check/test --no-run/build прошли из Linux — **compile/link, не execution**. MSVC
cross-check заблокирован C toolchain; macOS runner отсутствует. Четыре process
regressions теперь используют controlled Rust child вместо Unix shell/sleep.
[Команды, reports и ограничения](platform-verification.md). GUI запуск/явная
проверка отложены пользователем; GUI runtime/manual не PASS.

**P11-V01: полный Linux verification выполнен**, но весь P08/P11 не объявлен
завершённым. Проверенный core code — `9594de6`; последующее обновление V01 меняет
документацию. Core fmt/check/test/build: **571 library + 2 CLI + 2 integration +
18 doctests passed**, 0 failed, 1 ignored. Fresh GUI replay patches 01–05:
resolve/fmt/check/test/build PASS, **238 passed, 0 failed, 7 ignored**.
[Evidence, команды и ограничения](gui-migration/linux-verification.md).

- **P08:** все четыре byte domains используют общий ObjectStore и actual SHA-256,
  но физические namespaces раздельны. Startup collectors для content CAS,
  runtime generations/blobs, private asset CAS и managed homes/archives требуют
  complete owners/SQL/operation roots, process pins, age и повторяемое deletion
  intent/replay. Historical/unknown ownership, missing/replaced DB и corrupt/future
  metadata откладывают collection; Mojang SHA-1/virtual placements не собираются.
- **P09:** owned RuntimeRegistrar, record-local managed recovery diagnostics,
  common session JobId/Outcome, atomic active/completed reconciliation и bounded
  completion history. Shared Tasks закрывает admission, сигнализирует отмену и
  ждёт owned publishing/cleanup; running games не убиваются автоматически.
- **P10:** composition принадлежит `app`, host credentials передаются через Config,
  actual instances resource имеет lifetime lock, paths разрешаются без eager home
  fallback. Typed runtime sources отделены от durable diagnostics; CLI parsing/help
  выполняются до dotenv/config/bootstrap.
- **P11 A07/G01–G04:** low-level SQL/registry/install/recovery capabilities и shims
  закрыты, external positive consumer и 18 negative rustdoc contracts проверены.
  Изолированный GUI принимает credentials, ждёт normal-close shutdown, восстанавливает
  операции по typed subjects, сохраняет managed reports и предоставляет confirmed
  content recovery. Mods catalog остаётся отключённым; recovery не запускает поиск.

Остаются частичными **B02, B06, D08, S05, S06**: historical hardlinks/provenance,
недостающие dependency metadata, ownership/placement migration и более агрессивный
GC не угадываются автоматически. **P11-V02 частичен:** native Windows/macOS, manual isolated
launch/install/content, реальные JVM/descendants, live providers, external concurrent
FS/DB replacement и power-loss не получили runtime evidence. Abnormal GUI process
termination не превращается в cooperative close. Исторические string UI adapters
вне Java runtime/settings и content recovery не объявлены полностью мигрированными.

Исходный GUI и его dependency/lockfile не изменены этой миграцией. В финальном replay
проверено сохранение 173 исходных GUI файлов и его Git status. Локальный core
`Cargo.lock` сохранён вне коммитов; автоматического переключения launcher и push нет.

[B06 follow-up](installed-resolver-contract.md) проверяет Required/Unknown связи
всех fixed installed seeds в solver и catalog traversal. `NULL` dependencies
блокируют план typed error, `[]` остаётся известным пустым списком, corrupt JSON
отвергается. Modified/disabled/missing/ambiguous файлы не переносят metadata.
25 новых graph/catalog/SQLite-files regressions; Linux core fmt/check/test/build:
615 library + 2 CLI + 2 integration + 18 doctests PASS, 0 failed, 1 ignored.
Historical metadata/unknown identity и manual/native P11 по-прежнему partial;
это не разрешение более агрессивного GC и не гарантия live mod compatibility.

**D08 follow-up (asset index roots):** fail-closed исправление private asset CAS
collector — строгий typed schema с уникальными object names, rejection первого
duplicate root, unknown types и явного `null`, проверка до `Collection::plan`.
5 новых regressions: red run 1 PASS / 4 FAIL (только valid control), green focused
5 passed, retention 17 passed; Linux полный suite — **620 library + 2 CLI + 2
integration + 18 doctests passed, 0 failed, 1 ignored**; Windows GNU compile/link
PASS без execution. D08 остаётся «Частично»: ownership/deletion не расширены, GUI
не запускался. [Контракт, регрессии и evidence](asset-index-retention-contract.md).

## История реализации P08/P09

Ниже сохранены evidence отдельных commits. Числа tests и фразы «следующая работа»
относятся к соответствующей исторической точке, не к итоговому состоянию выше.

Продолжение P08 зафиксировано коммитами `0e97c9a` (durable preparation/leases),
`8752028` (process-crash контракт и subprocess fixtures), `31d9d2f` (ownership
partials), `7859409` (owned CAS import staging), `9be8fe8` (content staging cleanup),
`0493299` (полный атомарный/no-clobber PID marker) и `2d37a3d` (CAS admission).
На checkpoint Q05 полный Linux fmt/check/test/build прошёл: **533 passed, 0 failed,
1 ignored**. Отдельный shared/exclusive барьер canonical store защищает внутренние
content/runtime blob операции, включая import → SQL lease, recovery/completion
и URL index publication. Проверены aliases, реальные независимые процессы,
release после process exit/последнего клона, дубликат Unix descriptor, отмена до
preparing и удержание guard workers/SQL cleanup после drop caller/bootstrap.
Предыдущие process-exit/scratch/links/unwind/marker fixtures также прошли.
Это не закрытие всего P08: D08, S05 и S06 остаются частичными. D07 имеет проверенный
общий byte identity contract. Scratch cleanup
и admission внутренних CAS flows реализованы. `aef7a11` добавляет content CAS GC:
complete append-only каталог persistent DB identities до imports, fail-closed
preparing/journal/binding/lease gate, SQLite write-locks до unlink/metadata
completion и versioned deletion intents. Проверены несколько БД, unknown owners,
возраст, symlinks, новые roots при replay, drop caller и process exit на шести
FS/SQL границах. `dd96cdd` добавляет broad game pin до profile/legacy reads,
generation ownership до outputs, отменяемый workspace admission и direct-child
Forge pins; cleanup сохраняет work/maven после drop/unknown exit. Process-exit
fixture сохраняет живой child после смерти launcher; fake executable, не JVM.
`47dbe51` добавляет library owner catalogue до outputs, fail-closed current
profile/legacy generation/hash roots и общий LockedDatabases helper; known writer
не ждёт exclusive readmission во время чужой установки. Все content GC regressions
и 13 новых runtime gate tests прошли; один старый fixture требовал catalogue до
удержания workspace, исправлен и повторный полный прогон прошёл.
`150e1f2` добавляет versioned generation ownership до outputs и startup generation
GC (7 дней): complete snapshot → intent → quarantine rename → outputs → metadata
→ completion. 16 новых tests, process-exit шести границ, new-root restore,
drop caller/SQL locks, повторная age проверка и stale/live/unknown child ownership
прошли. Старые/неизвестные поколения сохраняются, не реконструируются по PID.
`8a38e9d` выделяет общий Collection; `9659851` добавляет startup runtime blob GC
после generation maintenance. Current/legacy SHA-1 и bytes всех surviving owned
generations — roots; live/unknown/future preparations и pending/unknown quarantine
блокируют область. 18 новых blob tests и прежние content/generation regressions
прошли, включая startup integration, четыре process-exit границы и drop caller
во время blocking intent. `a69c75a` добавляет root-aware portability legacy library
snapshots и writers, production legacy launch разрешает relative paths через
linked_libs_at. Семь новых regressions прошли; external/unknown/unsafe paths не
угадываются и не переписываются. `081230b` подключает assets к общему ObjectStore:
Mojang SHA-1 layout остаётся изолированными placements; legacy import и verified
CAS repair не требуют сети. Девять новых tests прошли, включая drop worker caller,
process-exit import→SQL и production sync. Startup asset maintenance пока scratch-only,
без published GC на этой исторической точке. `534b79d` добавляет private asset CAS
GC (7 дней) с complete owner DB/SQL/index/placement roots и owned replay; 12 новых
retention tests прошли. Mojang SHA-1 placements/SQL/virtual projections не удаляются,
поэтому Minecraft не требует private CAS pin; unknown owners/layouts/pending imports
откладывают collection. `b5bc1e7` добавляет managed Java operation ownership,
matching-marker staging cleanup и shared writer/exclusive recovery admission;
per-operation lock и durable reread защищают от stale validation. 16 новых tests
прошли, включая four process-exit boundaries, отдельный live writer process и
drop start/validation/recovery caller. Legacy/unknown/live ownership не присваивается.
Следующая работа: managed Java archive identity/SQL portability/roots/process pins;
published homes пока не собираются. `2c38a4b` добавляет managed Java SQL portability:
root-aware writers/readers и owned upgrade без смены IDs/FK/bytes/DB identity.
External/unknown/old-root/conflicting pairs не присваиваются. 13 новых tests прошли,
включая relocation, SQL process-exit boundaries и drop caller/SQL probe. Дальше —
published-home roots/pins; Q04/Q05 остаются P09. `b63e623` подключает managed
archives к общему immutable ObjectStore: actual SHA-256/exact size, cache install
без transport, независимые homes и admission в blocking importer/extractor.
10 новых tests и пятая ownership process-exit граница прошли. `c35d25e` исправляет
выявленную full run гонку raw lifetime locks с inherited descriptor; deterministic
regression и повторный full run прошли. Published Java archives/homes пока не GC.
`18f3c37` добавляет managed owner catalogue до writers, independent versioned home
marker до publication и protected multi-DB SQL/operation/home archive roots.
Historical/ephemeral/unbound coverage не объявляется complete; unknown/future
records/layouts, pending scratch и missing/replaced owners откладывают всю область.
12 новых regressions, caller-drop SQL probe и sixth process-exit boundary прошли.
Startup inspect не удаляет bytes/homes; process pins и GC остаются следующими.
`c9b8402` реализует production managed Java pins для selection/launch/install,
discovery и validation/Forge children. Broad operation и direct child имеют
независимые durable records: confirmed child exit не отпускает outer reads/commit,
outer completion не подтверждает unknown child. Game удерживает Java и libraries.
13 новых regressions и полный Linux прогон прошли; published Java GC ещё выключен.
На следующей точке `d364886` включён startup managed home/archive GC (7 дней):
catalogued dead-owned unrooted homes → replayable quarantine → новый archive
snapshot. Все owners/SQL/operations и surviving homes сохраняют source archives;
pending quarantine блокирует archives. Replay перепроверяет roots/pins/age,
restoration не заменяет existing home. Owned worker держит оба maintenance и DB
locks до cleanup/pool close после caller drop. 16 новых regressions, десять
subprocess deletion boundaries и startup integration прошли. Historical/unknown
ownership и asset SHA-1/virtual placements консервативно сохраняются, более
агрессивный GC не реализован. Следующий пакет — P09 registrar/recovery/history/shutdown.
P09 начат: `3d49bc3` вводит owned RuntimeRegistrar port; managed validation больше
не извлекает owning JavaService/registry/bus. Registration/lookup/notification
adapter находится в JavaService, Ready notification следует durable record commit.
GlobalState facade и deprecated compatibility call shape сохранены. Три новых
tests и прежние retained validation/caller-drop regressions прошли; далее Q05
independent corrupt-record diagnostics, затем bounded history/supervision/shutdown.
`8f9a953` изолирует managed recovery по records: 1 MiB regular reads, schema/path
validation и installations_report с diagnostics. Bad records и unknown scratch
сохраняются, поэтому GC по-прежнему fail-closed; healthy installations доступны.
Четыре новых regressions и обновлённые unsafe-home fixtures прошли; далее history.
Exclusive capability не доказывает отсутствие roots. Публичные low-level
raw-path APIs не дают автоматического pin; произвольный concurrent GC запрещён.
CAS types и content object storage теперь internal, остальные API — работа P11.
Power-loss durability не обещается. Windows/macOS, живые API и GUI/manual flows
не проверены. Исходное локальное изменение Cargo.lock не включено в коммиты.

В копии защищены существующие предки новых файловых путей, новые размещения модов используют копии, cache-hit записывается атомарно. Кеш игровых ресурсов проверяет фактический SHA-1. Запуск и переустановка экземпляра учитывают незакрытый журнал контента.

Файловая проверка относится к существующим предкам на момент проверки. Защита от одновременной замены каталогов и проверка на Windows остаются открытыми. Ранее созданные hardlink-размещения потребуют миграции. Размещение, которое recovery создало заново, записывается как copy. Совпадающий файл, найденный на месте при восстановлении, учитывается как внешний. Его способ размещения по содержимому определить нельзя.

Восстановление изолировано по экземплярам. Ядро читает журналы старого формата и записывает новые с версией 1. Если откатиться на старую версию ядра, она не сможет восстановить операцию, которую новая версия оставила незавершённой. Публичные методы ядра показывают незавершённые операции и допускают повтор после устранения конфликта. P11-G04 подключил этот API к изолированному GUI: confirmed abandon сохраняет файлы, Retry не требует прямой правки SQL.

Запуск по установленному профилю проверяет основной номер версии Java и известную архитектуру до файловых операций. Каталоги исключают недоступные релизы из подбора. Все запросы при составлении плана входят в общий лимит. Точный файл CurseForge подтверждается в пределах четырёх страниц списка с фильтром по загрузчику и версии Minecraft. Основание для фильтра описано в [REST-документации CurseForge](https://docs.curseforge.com/rest-api/#get-mod-files).
