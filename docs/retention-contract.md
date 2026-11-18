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
   участвовать в одной координации ресурса. Сейчас OS-lock ContentService
   защищает только соответствующий content store; runtime blobs не участвуют.
3. `preparing` удерживает **весь store**, в том числе ещё не зарегистрированные
   bytes в окне filesystem import → SQL registration. Проверка preparations и
   начало удаления не могут иметь окно, в которое допускается новая подготовка.
   Очистка чужого/неизвестного preparation key не заменяет координацию.
4. Чтение существующего объекта должно резервировать его до проверки/копирования.
   Новые imports должны передавать владение atomically в object lease/root прежде,
   чем освобождать store-wide защиту. Уже реализованные content SQL leases —
   часть этого контракта, но не полноценный concurrent-GC барьер.

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

18 ноября 2026 на Linux прошли `bash docs/check.sh fmt -- --check`, `check`,
`test`, `build`: **321 passed, 0 failed, 1 ignored**. Ignore — прежний тест Java
на PATH. Остались прежние unused Prism DTO warnings и future-incompatibility
`proc-macro-error2`. Логи:
`/tmp/opencode/core-retention-final-{check,test,build}.log`.

Эти доказательства закрывают только реализованную scratch cleanup, а не условия
будущего GC опубликованных данных из предыдущего раздела.
