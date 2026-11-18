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
flows держат operation guard, но публичные low-level APIs/raw paths ещё не закрыты.
Assets, managed Java и runtime generations пока не подключены к этому admission.

## Обязательные условия будущего GC — пока не реализованы

Ни SHA-256 filename, ни отсутствие SQL binding в одной БД, ни возраст файла сами
по себе не дают права удалять опубликованный объект или runtime generation.
CAS контента и runtime blobs используют общий bytes-код, но **разные каталоги**.
Полного общего реестра roots сейчас нет, поэтому произвольный concurrent GC
опубликованных данных запрещён.

### Область ресурса и admission

1. Collector должен установить фактический canonical root и область всех
   обслуживаемых каталогов/БД. Symlink aliases и `Config.dirs` overrides не создают
   независимое владение общим ресурсом. Если нельзя доказать полноту каталогов
   (например, один store разделяют разные БД), сборка откладывается.
2. Все импортёры, cache readers, registration, recovery и collector должны
   участвовать в одной координации ресурса. Внутренние content/CAS runtime blob
   flows используют описанный выше admission; collector, assets, generations и
   managed Java ещё не реализованы/подключены. Lifetime lock ContentService не
   заменяет admission и не доказывает полноту roots.
3. `preparing` удерживает **весь store**, в том числе ещё не зарегистрированные
   bytes в окне filesystem import → SQL registration. Проверка preparations и
   начало удаления не могут иметь окно, в которое допускается новая подготовка.
   Очистка чужого/неизвестного preparation key не заменяет координацию.
4. Чтение существующего объекта должно резервировать его до проверки/копирования.
   Новые imports должны передавать владение atomically в object lease/root прежде,
   чем освобождать store-wide защиту. Уже реализованные content SQL leases —
   часть этого контракта. Operation admission закрывает живое окно import → lease,
   но durable gate после drop/process exit и полный каталог roots ещё отсутствуют.

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

Эти доказательства закрывают только scratch cleanup и описанный admission
внутренних CAS flows, а не durable roots gate/GC опубликованных данных.
