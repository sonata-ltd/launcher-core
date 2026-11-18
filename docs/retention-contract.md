# Контракт удержания данных P08

## Реализованная область очистки

Очистка scratch не является GC опубликованных объектов. Сейчас можно удалять
только приватные временные данные с проверяемым владельцем:

| Данные | Владение | Очистка |
|---|---|---|
| Downloader partial | `<partial>.sonata-owner`, PID записан до файла | Подтверждённо завершившийся PID и минимальный возраст |
| CAS import staging | `staging/import-*/.sonata-owner` | Подтверждённо завершившийся PID; только непосредственные каталоги `import-*` |
| Content staging | `<instance>/.sonata-content-*/.sonata-owner` | Подтверждённо завершившийся PID; только непосредственные каталоги экземпляра |
| Natives | Owner PID, spawn intent, child PID | После intent нужны подтверждённо завершившиеся owner **и** child |

PID записывается полностью и синхронизируется до создания полезных scratch-файлов.
Для partials и owned staging общий `process::write_owner` публикует marker
атомарно через no-clobber: незавершённый числовой префикс PID не виден sweeper,
существующий чужой marker не перезаписывается.
Падение между созданием каталога и завершением marker оставляет неизвестный
каталог, который сохраняется. Нормальное завершение, ошибка, отмена и unwind
убирают принадлежащий RAII-объекту scratch. Process exit оставляет marker для
следующего запуска. Проверка существования PID не посылает сигнал процессу.
Повторное использование PID, отказ доступа или неподдерживаемая платформа
означают «возможно жив» и откладывают очистку. Возраст не доказывает abandonment.

Неизвестные namespaces, старые файлы/каталоги без markers, некорректные markers,
symlink roots/children/markers сохраняются. Очистка предполагает стабильные
предки; конкурентная подмена каталогов внешним writer не является поддерживаемым
sandbox-сценарием. Отдельный import staging не выдаётся JVM и не используется
SQL recovery. Content recovery читает CAS и конечные файлы, а не оставшиеся
staged-копии. Поэтому оба staging можно очищать независимо от publishing journal,
но **нельзя** одновременно удалять защищённый им CAS.

Content import/staging очищаются при создании ContentService под его существующими
блокировками. Runtime import staging очищается в существующем startup maintenance.
Владение PID допускает одновременную живую загрузку даже через другой alias root;
общий GC из этого не следует. Старый плоский CAS staging сохраняется.

## Реализованный admission/maintenance барьер

Общий `utils::object_store` использует `.sonata-access.lock` фактического canonical
store root. Shared OS-lock допускает параллельные readers/importers; exclusive
`StoreMaintenance` исключает их до завершения обслуживания. Symlink alias root
участвует в той же блокировке. Это отдельный ресурс, не замена lifetime ownership
lock ContentService (`.sonata-store.lock`) и не глобальная блокировка launcher.

- Content installation получает shared admission **до** SQL `preparing` и держит
  его до completion/discard. Импорт и SQL registration/lease проходят под тем же
  guard; проверка cache, placement и файловая публикация передают клоны workers.
- Recovery получает admission до чтения журнала и удерживает его до SQL completion;
  copy/verify/publish worker владеет отдельным клоном.
- Runtime `transfer::place` удерживает admission при чтении CAS/legacy blob и URL
  index, импорте, копировании и публикации нового index. Content и runtime stores
  используют один механизм, но остаются разными физическими ресурсами.
- Ожидание shared admission отменяемо. Отмена до допуска не создаёт preparation
  или scratch; синхронные low-level import/alias helpers возвращают `WouldBlock`
  при exclusive maintenance и защищены только на время своего вызова.
- Startup удаляет preparations своего canonical root только при exclusive
  maintenance. SQL cleanup выполняется отдельной задачей, владеющей capability:
  drop bootstrap-future не освобождает барьер раньше завершения запроса.
- Последний RAII-владелец явно снимает OS-lock. Закрытия одного descriptor
  недостаточно при временном наследовании/дублировании Unix file description;
  process exit освобождает блокировку средствами ОС. Ожидание не обещает fairness.

**Exclusive admission не означает, что данные можно удалять.** После drop/panic
операции durable preparation или publishing journal/leases могут остаться.
Maintenance capability сама не проверяет их и не является разрешением на GC.
`StoredObject` — verified snapshot, не pin; `path_for` только вычисляет layout.
Возвращённый raw path/его копия не продлевают admission helper. Внутренние рабочие
flows держат operation guard. CAS types и content object storage теперь internal;
публичные API других доменов/raw paths ещё предстоит закрыть в P11.
Workspace установки и сервисный запуск держат отдельный shared admission
canonical library root. Assets и managed Java пока не подключены к нему.

## Обязательные условия GC — content реализован, runtime остаётся открытым

Ни SHA-256 filename, ни отсутствие SQL binding в одной БД, ни возраст файла сами
по себе не дают права удалять опубликованный объект или runtime generation.
CAS контента и runtime blobs используют общий bytes-код, но **разные каталоги**.
Content CAS использует отдельный complete owner catalogue и gate, описанные ниже.
Полного runtime реестра roots пока нет; произвольный concurrent GC опубликованных
данных вне проверенной content области запрещён.

### Область ресурса и admission

1. Collector должен установить фактический canonical root и область всех
   обслуживаемых каталогов/БД. Symlink aliases и `Config.dirs` overrides не создают
   независимое владение общим ресурсом. Если нельзя доказать полноту каталогов
   (например, один store разделяют разные БД), сборка откладывается.
2. Все импортёры, cache readers, registration, recovery и collector должны
   участвовать в одной координации ресурса. Внутренние content/CAS runtime blob
    flows используют описанный выше admission; content collector подключён,
    assets, generations и managed Java — ещё нет. Lifetime lock ContentService не
   заменяет admission и не доказывает полноту roots.
3. `preparing` удерживает **весь store**, в том числе ещё не зарегистрированные
   bytes в окне filesystem import → SQL registration. Проверка preparations и
   начало удаления не могут иметь окно, в которое допускается новая подготовка.
   Очистка чужого/неизвестного preparation key не заменяет координацию.
4. Чтение существующего объекта должно резервировать его до проверки/копирования.
   Новые imports должны передавать владение atomically в object lease/root прежде,
   чем освобождать store-wide защиту. Уже реализованные content SQL leases —
   часть этого контракта. Operation admission закрывает живое окно import → lease.
   После drop/process exit durable gate content CAS реализован ниже; runtime
   roots/generations/JVM пока требуют отдельного gate.

### Roots опубликованных объектов

- Все `instance_content.object_id`, включая disabled, pinned, missing и unstable:
  отсутствие файла не означает отказ пользователя от сохранённой записи.
- Все object leases и publishing journals. Неизвестный/повреждённый journal
  блокирует затрагиваемую область GC; нельзя молча трактовать его как пустой.
- Текущие InstalledProfile и legacy library placements с их путями, ролями и
  порядком. Cache aliases/URL indexes — hints, не semantic roots; их обслуживание
  требует собственного проверяемого протокола.
- Durable pins всех живых JVM/подготовок запуска, а также installer JVM. Root
  определяется до чтения/проверки профиля и сохраняется при завершении launcher.
  Живой процесс может лениво открыть JAR позже: отсутствия открытого descriptor
  или in-memory RunningRegistry недостаточно.
- Исторические и неизвестные владельцы не удаляются автоматически. Managed Java
  требует отдельного учёта journal/registration/process roots; внешние Java paths
  не принадлежат launcher и никогда не становятся целью его GC.

### Runtime generations

Создание поколения должно записывать ownership до runtime outputs. Текущий
профиль удерживает своё поколение. Запуск должен получать durable generation pin
до чтения профиля/артефактов, записывать spawn intent до spawn и child PID после
него. После intent с неизвестным PID поколение сохраняется. Это касается как
игры, так и внешних installer processors. После переустановки старое поколение
может оставаться нужным живой JVM; PID старого launcher не является её владельцем.

Collector допускается только после доказательства отсутствия текущих профилей,
активных preparations и live/unknown pins. Неизвестные старые поколения или
нечитаемые/неподдерживаемые профили сохраняются. Удаление регистрации экземпляра
сейчас оставляет game files, поэтому само по себе также не разрешает их очистку.

### Реализованные runtime process pins (ещё не разрешение GC)

Коммит `dd96cdd`; полный Linux fmt/check/test/build — PASS:
**370 passed, 0 failed, 1 ignored** (12 новых tests и расширенная watcher
regression). Проверены owner до outputs, cancelled admission без generation,
workspace cleanup при live/unknown pins, fake processor success/failed/cancel
и drop future, watcher acknowledgement до Exited event, marker/version/link/PID
fixtures. `process_exit_preserves_live_child_and_unknown_spawn_intent` завершает
launcher через exit(75), сохраняет живой OS child и intent без известного child
PID. Логи: `/tmp/opencode/core-runtime-pins-verified-{check,test,build}.log`.
Ignore, прежние Prism warnings и proc-macro-error2 не изменились.

`utils::process_pin` записывает UUID-каталог `.sonata-processes/<uuid>` с полным
owner PID до возврата capability. Versioned spawn intent предшествует spawn;
полный child PID записывается no-clobber после spawn. `.sonata-exited` появляется
только после подтверждённого выхода и окончания сопровождающих owned workers.
Все markers публикуются полностью через synced tempfile/no-clobber. У pin нет
автоматического удаления при Drop: kill-on-drop не является exit acknowledgement.
Unknown entries/versions/PIDs, symlinks и PID reuse сохраняют данные.

Сервисный launch получает shared admission canonical library root и **широкий
pin всего library store до чтения InstalledProfile/legacy inputs**. После spawn
pin передаётся watcher вместе с natives и подтверждает завершение до Exited event.
Если launcher погиб, marker остаётся: owner PID сам по себе не разрешает очистку,
когда child ещё жив или его PID неизвестен. Широкий pin сознательно не пытается
восстановить минимальный набор лениво открываемых JVM библиотек; будущий collector
должен сохранять всю pinned library область.

Workspace получает отменяемый shared library admission до создания поколения и
записывает `.sonata-owner` до maven/work outputs. Forge processors создают отдельные
pins **в корне поколения**, а не в extracted work. Подтверждённый success, failed
exit или kill+wait разрешают завершение pin после diagnostics/verification;
drop будущего не пишет `.sonata-exited`, даже если kill-on-drop уже отправлен.
`cleanup_failed` и `cleanup_work` требуют проверяемого собственного поколения и
отсутствия live/unknown process pins; иначе оставляют maven/work. Неизвестный child
не разрешает очистку при живом или погибшем launcher. Ссылки в ancestors нового
поколения отвергаются до outputs; cleanup не проходит через process-pin links.

Это пока не полный runtime lifecycle: generation/blob deletion protocol и старые
generation ownership identities ещё отсутствуют. Runtime GC остаётся выключен.
Неподдерживаемые старые
writers, конкурентная внешняя подмена ancestors и оторвавшиеся descendants
внешнего процессора не покрываются обещанием direct-child ownership. Managed
Java и assets требуют отдельных roots/pins. Проверки используют реальные OS
subprocesses и fake processor executable, **не** живые game/installer JVM.

### Runtime catalogue и current roots gate

Коммит `47dbe51`; полный Linux fmt/check/test/build — PASS:
**383 passed, 0 failed, 1 ignored**. Все content collector regressions прошли с
общим LockedDatabases helper; добавлены 13 runtime gate tests. Старый reinstall
fixture, создававший workspace до catalogue, зависал на правильном exclusive
gate; fixture исправлен без ослабления production admission, повторный полный
прогон прошёл. Логи: `/tmp/opencode/core-runtime-gate-final-{check,test,build}.log`.

Library root теперь получает собственный append-only `.sonata-roots.json` до
создания workspace и до запуска. Bootstrap и новые владельцы используют owned
attach task: drop caller не освобождает admission до завершения записи каталога.
Уже catalogued writer проверяет identity/coverage под shared admission, поэтому
параллельная установка не требует повторного exclusive admission. Неизвестный
owner durably снимает completeness; повреждённый/будущий catalogue не допускает
нового writer. Catalogue, созданный поверх старых bytes, не становится complete.

Runtime snapshot исключает broad live/unknown game pins и через общий
`data::db::retention::LockedDatabases` открывает всех persistent owners без create,
сверяет identity/path и удерживает SQLite `BEGIN IMMEDIATE` вместе с library
maintenance. Этот helper используется и проверенным content collector; доменные
roots остаются в соответствующих модулях.

Snapshot сохраняет поколения и SHA-1 **всех** текущих profile artifacts независимо
от classpath/native и наличия файлов. Profile version, неизвестные root/artifact
поля, canonical `instances/<instance-id>/<uuid>/maven`, относительные paths и
SHA-1 проверяются fail-closed. Учитываются instance_library_placements и связанные
старые instances_libraries snapshots, включая missing files. Legacy absolute path
принадлежит только фактическому library root; перенос без доказанного владельца
блокирует gate. Digest/URL aliases не становятся semantic roots.

В `47dbe51` startup выполнял только diagnostic snapshot, без удаления поколений
или runtime blobs. Проверены persistent SQLite двух owners, current/legacy roots,
ошибочные версии/fields/paths/hashes, historical/ephemeral/missing/replaced owners,
catalogue до workspace outputs, повторный known-writer attach во время установки,
cancelled admission, broad game pins, canonical aliases/escaping generation links,
SQL/maintenance lifetime и drop attach caller. Runtime collector должен владеть
snapshot всей owned задачей; передача одного raw path не создаёт retention pin.

### Versioned generation ownership и quarantine/replay

Коммит `150e1f2`; полный Linux fmt/check/test/build — PASS:
**399 passed, 0 failed, 1 ignored**. 16 новых tests, включая настоящие subprocess
создания workspace и exit без destructors; process-exit collector на intent,
rename, outputs, metadata, removed и cleaned; new current root после intent/rename;
replay fresh outputs/age; caller drop с SQLite lock probe; другая catalogued DB и
legacy placements; live/unknown child после смерти owner; unknown/future/replaced
ownership и links. Логи: `/tmp/opencode/core-generation-gc-verified-{check,test,build}.log`.
Прежние ignore/Prism/proc-macro-error2 warnings не изменились. Реальные JVM,
Windows/macOS и power-cut не проверялись.

Новые production workspaces проверяют catalogue coverage после shared admission
и записывают `.sonata-generation.json` до maven/work outputs. Record содержит
version 1, instance/UUID, persistent DB identity (либо unknown для ephemeral DB),
полный owner PID и creation time. UUID directory создаётся no-clobber; совпадение
PID в чужом существующем каталоге не даёт права удалить его при ошибке создания.
Ошибка owner read теперь возвращается **до** generation outputs.

Generation collector использует complete runtime snapshot всей owned задачей:
library exclusive maintenance и SQL locks всех DB сохраняются до окончания
blocking rename/remove. Кандидат должен иметь matching layout/record/PID и
catalogued persistent identity, подтверждённо отсутствующего launcher owner,
отсутствие live/unknown process pins, известные top-level entries и только
regular descendants. Возраст **7 дней** проверяется по record creation и всем
оставшимся файлам, а не только по возрасту каталога. Future timestamps,
unknown/corrupt ownership, чужие top-level files и links сохраняются. Старые
поколения с одним PID marker не получают ownership автоматически.

Versioned `.sonata-runtime-gc/generations/<uuid>.json` сохраняет ownership до
rename в одноимённую quarantine directory. Только затем удаляются maven/work
и завершённые process pins; ownership metadata удаляется последней, directory и
intent — после outputs. Replay снова выполняет current roots/owners/process/age
gate и проверяет original/quarantine ancestors. Replacement owner или неизвестная
quarantine не разрешают unlink. Если до replay появляется current root, ещё
сохранившиеся проверяемые quarantined bytes возвращаются без overwrite; это не
обещание восстановить bytes, уже удалённые до crash. Новое поколение никогда не
публикуется с повторно использованным UUID существующего workspace.

В `150e1f2` generation GC запускается на startup; blob collector добавлен отдельным
проверенным шагом ниже. Assets и managed Java не затрагиваются. API unregister сам по себе не означает удаление
game files. Гарантия остаётся process-crash, не power-loss; concurrent внешняя
подмена filesystem/DB, старые writers и detached descendants не поддерживаются.

### Runtime blob collection

`8a38e9d` выделяет neutral `object_store::collection::{Collection, Roots,
DeleteIntent}`: versioned intents, проверка bytes/age при replay, unlink и cleanup
только matching SHA-1 alias. Domain SQL completion/identity/provenance не переезжает
в FS helper. Все прежние content/generation regressions прошли (399 tests).

`9659851` добавляет runtime blob collector и startup-порядок generation → blobs,
возраст 7 дней. Owned task удерживает complete runtime snapshot/все SQLite
BEGIN IMMEDIATE и exclusive library admission; blob maintenance берётся вторым,
через try, чтобы не ждать standalone importer под SQL locks. Оба admission и SQL
живут до окончания blocking root scan/plan/unlink/intent cleanup, даже после drop
caller. Каждый replay строит roots заново.

Semantic roots — SHA-1 всех current profile artifacts и обоих legacy snapshots,
даже missing/не-classpath files; дополнительно хешируются regular maven/work bytes
**каждого сохранившегося** versioned generation с known catalogued owner.
Snapshot не угадывает будущие inputs: live/unknown owner/child, unknown/future
metadata/timestamps/layouts, linked/special files, неизвестные top-level library
entries и pending/unknown quarantine блокируют всю blob-область. Completed
generation с ещё живым launcher PID также консервативно блокирует blobs.

Удаляются только проверенные canonical SHA-256 objects. Aliases и URL indexes —
hints, не roots; stale URL hints и flat legacy SHA-1 blobs не удаляются, consumers
обязаны проверять bytes. SQL identity/provenance collector не меняет.

Полный Linux fmt/check/test/build — PASS: **417 passed, 0 failed, 1 ignored**;
18 новых tests, все предыдущие content/generation regressions. Проверены четыре
process-exit границы intent/unlink/alias/cleaned, new profile roots и fresh reimport
age при replay, drop caller на snapshot и blocking intent с SQL probes, independent
blob admission, two DB/legacy/current/missing roots, surviving maven/work,
live/unknown/future preparations, canonical aliases, links, pending/corrupt/unknown
quarantine и startup generation→blobs. Логи:
`/tmp/opencode/core-blob-gc-verified-{check,test,build}.log`.
Ignore и прежние Prism/proc-macro-error2 warnings сохранены; новые warnings не
появились. Assets/managed Java, реальные JVM, detached descendants, Windows/macOS,
GUI/manual и power-cut ещё не подтверждены. Process-crash — не power-loss гарантия.

### Legacy library path portability

`a69c75a`: startup attach → owned root-aware path upgrade → generation → blob GC.
Migration удерживает library maintenance и SQLite BEGIN IMMEDIATE до commit после
drop caller. Меняются только path columns обоих legacy snapshots: absolute paths
под admitted canonical/configured root становятся безопасными relative placements.
Missing file не мешает доказанному root ownership; escaping ancestor, traversal,
external или неизвестный старый root не переписывается. JSON профиля, files,
roles/positions/links и database identity не меняются. Retry идемпотентен, errors
откладывают upgrade. Никакой schema/version migration или guessing прежнего root.

Новые root-aware legacy writers сохраняют relative placements; outside/unsafe
paths отвергаются до transport. Legacy launch использует `linked_libs_at`, который
разрешает relative paths относительно текущего library root и проверяет hash/path.
Старый `linked_libs` deprecated, возвращает raw snapshot (включая relative paths),
не executable-path capability и не pin. Admission/pin — ответственность caller.

Полный Linux fmt/check/test/build: **424 passed, 0 failed, 1 ignored**. Семь новых
regressions проверяют bytes/roles/positions/associations/JSON/identity, idempotence,
relocation, missing/external/unsafe paths, canonical alias и escaping ancestors,
relative writer/early rejection, drop caller и invalid reader input; прежний
workspace portability test также прошёл. Логи:
`/tmp/opencode/core-library-paths-verified-{check,test,build}.log`.
Managed Java SQL paths ещё не мигрированы. Historical catalogue DB paths не
переносятся автоматически: missing/replaced DB по-прежнему откладывает GC.

### Asset byte identity (collection not yet enabled)

`081230b` использует общий ObjectStore в `assets/objects/.sonata-cas` для immutable
SHA-256 identity. Обязательный Minecraft `objects/<sha1-prefix>/<sha1>` остаётся
placement: valid legacy bytes импортируются с SHA-1 проверкой во время копирования,
SQL IDs не меняются. Missing/corrupt regular placement можно атомарно восстановить
из независимо проверенного CAS; aliases и SQL не cache proof. Copy размещение не
позволяет изменением asset file изменить CAS. Corrupt authoritative object не
перезаписывается; special/linked/escaping placements отклоняются. Downloaded bytes
верифицируются/импортируются до register SQL, replay идемпотентен.

Admission принадлежит blocking cache/import worker и живёт до его завершения после
drop caller. Startup sweep удаляет только dead-owned import scratch в отдельном
asset CAS; published objects, aliases, Mojang placements и SQL rows не удаляются.
Это промежуточный identity контракт: полного asset roots/catalogue/pins gate ещё
нет, published asset GC намеренно выключен. Raw paths/snapshots не retention pins.

Полный Linux fmt/check/test/build: **433 passed, 0 failed, 1 ignored**. Девять новых
asset tests плюс два прежних regressions: read-time import/SQL identity, независимые
copies, relocation, SQL/alias hints/corrupt bytes, early registration rejection,
cancellable admission, escaping links, drop caller в blocking worker, process exit
после import до SQL и replay без duplicates, production sync repair без HTTP.
Логи `/tmp/opencode/core-assets-cas-verified-{check,test,build}.log`.

### Conservative asset CAS collection

`534b79d` добавляет startup attach → scratch sweep → private CAS collection (7 дней).
Writer attach предшествует bytes; complete append-only catalogue имеет те же
immutable DB identities и historical/unknown/ephemeral fail-closed правила, что
content/runtime. Старый CAS без catalogue никогда не объявляется completely owned.
Owned collector удерживает canonical CAS maintenance и все owner SQLite
BEGIN IMMEDIATE до окончания blocking scan/plan/unlink/intent cleanup после drop
caller; replay каждый раз строит новый snapshot, повторяет roots/age gate.

Roots — консервативный superset: hashes всех asset SQL cache rows всех owners,
всех regular asset indexes и всех canonical Mojang SHA-1 placements. Даже corrupt
placement сохраняет предназначенный checksum для repair. Aliases не roots/proof.
Unknown/future index/object fields, layouts, pending import scratch, links,
corrupt metadata и unavailable/replaced DB откладывают всю private CAS область.
Virtual/resource layouts пока unknown, также откладывают GC.

Collector удаляет только verified old unrooted SHA-256 objects и matching aliases,
через общий Collection. SQL rows/identity, indexes и Mojang/virtual placements не
меняются и не удаляются. Minecraft читает изолированные SHA-1 copies, не CAS:
game retention pin для **этого ограниченного collector** не требуется. Import/cache
worker держит CAS admission до окончания copies/import; ещё не импортированный
network download не читает CAS и импортирует только после admission. Без ownership
migration исторических placements нельзя включить их GC или выводить отсутствие
roots только из одного профиля. Поэтому более агрессивный asset cleanup остаётся
открытым, а DB cache roots намеренно over-retain.

Полный Linux fmt/check/test/build: **445 passed, 0 failed, 1 ignored**; 12 новых
asset retention tests и все прежние regressions. Four process-exit boundaries,
new SQL/index/placement roots и fresh reimport age на replay, drop caller на
snapshot/blocking intent с SQL probe, multiple/missing/replaced DB, unknown owners,
metadata/layouts, canonical aliases, links, future bytes и startup integration.
Логи `/tmp/opencode/core-assets-gc-verified-{check,test,build}.log`.

### Managed Java staging ownership and recovery

`b5bc1e7`: `.managed` имеет общий canonical shared writer/exclusive recovery
admission. Start/install/validation — owned tasks; recovery blocking worker владеет
exclusive scope до cleanup/record commit после drop caller. Отдельный exclusive
operation scope запрещает двум initialized services одновременно писать одну
операцию. Validation перечитывает durable record под этим lock до присвоения:
старый AwaitingValidation/Failed snapshot не заменяет чужой active/Ready record.
Loaded active entry не даёт локального cancel, даже если owner PID равен текущему.

Record и staging `.sonata-owner.json` содержат version/operation UUID/PID. Новый
staging создаётся create_dir без adoption/clobber. Cleanup normal writer требует
local owner; recovery — доказанно dead owner, оба требуют совпадающий bounded
regular marker. PID reuse/permission/zero/future/unknown ownership сохраняет bytes.
Unmarked directory после exit до marker остаётся unknown. Terminal records могут
очистить dead-owned matching scratch после cleanup crash, но не published homes.
Mismatch/missing/corrupt marker откладывает эту операцию, record не переписывается;
normal failure сохраняет cleanup diagnostic. Legacy active owner не присваивается.

Root-aware проверки records/staging/home/executable предшествуют FS I/O и probe;
escaping executable не запускается. Filename обязан совпадать с record UUID.
Существующий publication destination отвергается; это не защита от конкурентной
внешней FS подмены. Recovery по-прежнему fail-fast для corrupt/unsafe самого JSON
record (Q05 isolation относится к P09). Shared admission не retention roots gate,
published homes/CAS GC здесь не включён; game/installer process pins ещё открыты.

Полный Linux fmt/check/test/build: **461 passed, 0 failed, 1 ignored**. 16 новых
tests и прежние 25 managed tests: four actual process-exit boundaries, отдельный
live writer process (admission, затем live PID без lock, затем confirmed exit),
drop start/validation/recovery caller, operation exclusion/stale durable record,
unknown/live/legacy/future owner, mismatched/missing/corrupt marker, canonical alias,
escaping FS/executable, record identity/unsafe homes и preexisting destination.
Логи `/tmp/opencode/core-managed-owner-verified-{check,test,build}.log`.
Это Linux synthetic executable/process-crash evidence, не реальные JVM/descendants,
Windows/macOS, power-loss или GUI/manual evidence.

### Порядок удаления и проверка

Нужен повторяемый протокол FS/SQL с admission barrier и повторной проверкой roots
перед удалением, а не «SELECT unreferenced, затем remove». После process crash на
каждой границе collector должен безопасно продолжать/откладывать очистку без
потери текущих, leased, live и unknown данных. Удаление последнего root и очистка
provenance/aliases не должны выдавать stale metadata за verified cache hit.

Обязательные regressions для расширения: import до registration, preparation
без известных IDs, publishing lease, disabled/missing/pinned binding, current
profile, живые game/installer JVM после смерти launcher, неизвестный child PID,
переиспользованный PID, другая БД/alias общего store, corrupted journal/profile,
legacy unknown ownership и повтор recovery/collection после process exit.

Power-loss durability этим контрактом не обещается. См.
[publication-contract.md](publication-contract.md); Windows/macOS runtime и
power-cut испытания остаются отдельными непроверенными критериями.

## Доказательства текущего шага

- `7859409`: shared owned import staging и startup cleanup для content/runtime
  stores. `process_exit_allows_import_staging_cleanup_without_collecting_bytes_or_live_owners`
  завершает отдельный процесс внутри реального import до publish; сохранены
  опубликованный объект/alias, живой импорт и legacy unmarked файл.
- `9be8fe8`: owned content staging и startup cleanup. Существующий
  `process_exit_recovers_each_publication_boundary_without_duplicate_links`
  проверяет реальные staged/intent checkpoints, удаление брошенного staging и
  recovery без него. `startup_keeps_live_content_staging_and_legacy_unknown_ownership`
  сохраняет живые/неизвестные каталоги. Ошибка, отмена и symlink fixtures также
  проверены; GUI и живые provider API для этого шага не использовались.
- `0493299`: общий complete-PID/no-clobber helper для scratch и partials.
  `owner_publication_is_complete_and_never_replaces_an_existing_marker`
  проверяет полный PID, сохранение чужого marker и отсутствие обычного temp leak.

Для scratch checkpoint `770321e` 18 ноября 2026 на Linux прошли
`bash docs/check.sh fmt -- --check`, `check`,
`test`, `build`: **321 passed, 0 failed, 1 ignored**. Ignore — прежний тест Java
на PATH. Остались прежние unused Prism DTO warnings и future-incompatibility
`proc-macro-error2`. Логи:
`/tmp/opencode/core-retention-final-{check,test,build}.log`.

Admission checkpoint `2d37a3d` добавляет **16** тестов. Основные доказательства:

- `imported_unregistered_bytes_stay_admitted_until_sql_lease_registration` —
  реальный import и SQL registration/lease: CAS bytes опубликованы, но object ID
  и leases ещё отсутствуют; exclusive maintenance недоступен.
- `installation_keeps_admission_through_publication_and_sql_completion` и
  `recovery_keeps_admission_until_completed_bindings_replace_journal_leases` —
  конечные файлы уже опубликованы, SQL bindings ещё не созданы; shared guard
  сохраняется до завершения перехода.
- `cancelling_admission_wait_does_not_create_a_preparation_or_staging` и runtime
  `cancelled_maintenance_wait_does_not_read_the_cache_or_touch_destination` —
  отмена до допуска без preparations, CAS чтения, index или destination changes.
- `dropping_the_caller_does_not_release_a_live_import_workers_admission` и
  `dropping_bootstrap_does_not_release_admission_before_the_sql_cleanup_finishes`
  — guard принадлежит продолжающему работу worker/cleanup, а не только caller.
- Независимые shared handles и клоны, alias root, независимые процессы с shared и
  exclusive locks и release после `exit(73)`, low-level contention до staging,
  symlink/directory lock rejection и Unix duplicated descriptor проверены.
  `startup_cannot_discard_preparations_while_a_reader_holds_admission` сохраняет
  preparation при живом shared owner, даже если object IDs ещё неизвестны.

Повторный полный Linux прогон 18 ноября 2026: `fmt -- --check`, `check`, `test`,
`build` — PASS, **337 passed, 0 failed, 1 ignored**. Предыдущие scratch/publication
fixtures также прошли. Ignore и warnings не изменились. Логи:
`/tmp/opencode/core-admission-final-{check,test,build}.log`.

Эти admission-доказательства сами по себе не разрешают GC опубликованных данных.

## Content CAS: durable roots gate и повторяемое удаление

Collector запускается на старте ContentService после recovery и миграции
зарегистрированных hardlinks, с минимальным возрастом **7 дней**. Он не работает
с runtime generations, runtime blobs, assets или Java. Все instance bindings
удерживают content bytes независимо от enabled/pinned/presence; копия мода в
экземпляре не используется как замена durable binding.

Миграция 0015 добавляет неизменяемую случайную identity SQLite. Append-only
`.sonata-roots.json` находится в canonical content store и регистрирует каждую
persistent БД до новых imports при exclusive maintenance. Alias того же store
использует тот же каталог. Первый каталог может быть complete только в пустом
store (не считая lock-файлов). Исторические данные без каталога, ephemeral DB и
owner, identity которого невозможно прочитать, навсегда снимают completeness;
автоматического восстановления исторического владения нет. Отсутствующая,
заменённая или несовместимая БД откладывает GC. Старые owners не забываются даже
после unregister. Если каталог нельзя безопасно обновить, bootstrap отказывает
в допуске нового content writer, а не пишет bytes вне ранее complete каталога.

Collector заново открывает **все** catalogued DB без create, сверяет canonical
path и identity и держит `BEGIN IMMEDIATE` transactions от snapshot до unlink
и SQL completion. Любой preparing, даже с чужим store key или без object ID,
блокирует весь content CAS. Publishing journals должны иметь известную версию,
canonical UUID, portable уникальные имена, существующую identity и lease на
каждый файл. Неизвестные/повреждённые records не становятся пустыми roots.

Удаление:

1. Под exclusive admission и SQL-locks выбираются только regular SHA-256 objects
   с корректным layout, digest и достаточным возрастом. Неизвестные имена,
   повреждённые bytes, symlinks и future timestamps сохраняются.
2. Versioned `.sonata-gc/<sha256>.json` фиксирует SHA-256, SHA-1 и size до unlink.
3. Перед unlink bytes повторно проверяются. Только SHA-1 alias с **этим** SHA-256
   удаляется; чужой alias не заменяется и не удаляется.
4. В каждой БД `store_state` становится `external`; identity, provenance и FK
   сохраняются. Intent удаляется только после commit всех catalogued DB.

После process crash replay заново выполняет весь roots gate. Новый binding
сохраняет bytes и снимает старый intent. Reimport тех же bytes заново проверяет
возраст; старый intent не разрешает удалить свежий объект. Отсутствующие bytes
не являются cache hit; replay завершает alias/SQL repair. Вся async collection
принадлежит отдельной задаче: drop caller не освобождает transactions во время
blocking unlink. Ошибки оставляют повторяемые intents и откладывают следующий
проход; не обещается атомарный commit нескольких БД и файловой системы.

Коммит `aef7a11`: проверена 21 новая regression/contract проверка (включая upgrade 0015), file
SQLite нескольких owners, historical/ephemeral/missing/replaced DB, preparation
без IDs, все binding states, publishing/лишний lease, corrupt/future journals и
intents, canonical alias, symlinks, возраст, provenance, drop caller с реальным
SQLite lock probe. `process_exit_replays_intent_unlink_alias_and_each_sql_commit_boundary`
проверяет intent → unlink → alias → SQL updates → первый commit из двух → cleanup
через `exit(74)` без destructors. Отдельный process-exit fixture добавляет live
binding перед replay. Это process-crash гарантия, не power-loss гарантия;
конкурентная внешняя подмена ancestors/DB и старые writers без admission не
поддерживаются. Windows/macOS и живые JVM для этого шага не проверялись.

Полный Linux fmt/check/test/build — PASS: **358 passed, 0 failed, 1 ignored**.
Ignore и прежние Prism/proc-macro-error2 warnings не изменились. Логи content GC:
`/tmp/opencode/core-content-gc-final-{check,test,build}.log`.
