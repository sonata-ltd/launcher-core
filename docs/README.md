# Подготовка полного рефакторинга ядра

Дата: 18 ноября 2026. Основание: [архитектурный аудит](core-architecture-audit-2026-11-17.md).

## Рабочая копия и исходная точка

Рабочий каталог: [core-refactor](../../core-refactor/Cargo.toml). Это самостоятельный Git-репозиторий на ветке `refactor/all-audit-findings`, содержащий снимок **текущего рабочего дерева** `/home/quartix/Projects/launcher/core`, включая незакоммиченный контент и SQLx metadata. История оригинального репозитория не клонировалась; его HEAD записан в manifest. Снимок — первая локальная контрольная точка копии: `69ca8df619b4`.

Скопировано 227 файлов, SHA-256 каждого проверен. Все 178 Rust-файлов совпали с инвентарём аудита. Скопированы Cargo.toml/Cargo.lock, src, migrations, .sqlx, .cargo, README/LICENSE/.gitignore и local_docs. Не копировались runtime/build данные: target, dev.db, .env с credentials, logs, .crush и исходный .git. Для тестов использовать временные БД/каталоги; рабочие данные launcher не нужны.

- `baseline-manifest.json` и `source-status.txt`: исторические материалы снимка; в этой рабочей копии отсутствуют.
- [TASKS.md](TASKS.md): полный реестр замечаний с пакетами реализации.
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

1. Один связный пакет — несколько небольших Conventional Commits в копии. Перед переносами модулей сначала зафиксировать поведение тестами. Не смешивать функциональное исправление, массовый rename и schema migration в одном коммите.
2. Каждый пункт TASKS должен получить проверку, реализацию и ссылку на commit/тест. Замечания аудита включают риски и рекомендации: сначала подтвердить актуальность, затем исправить дефект или явно определить и проверить контракт. Нельзя считать любой переезд файла устранением архитектурной проблемы.
3. Не редактировать применённые миграции 0001–0008. Новые constraints/форматы вводить следующими миграциями с upgrade fixtures. Journal версии 0 читать до перехода на версию 1; unknown schema возвращает диагностику конкретной операции.
4. До сужения pub API сохранить compile fixtures потребителей GUI/CLI. При переносах предусмотреть временные deprecated re-exports; старые импорты внутри ядра перевести сразу. Удалять compatibility API только после миграции реальных потребителей.
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

В копии защищены существующие предки новых файловых путей, новые размещения модов используют копии, cache-hit записывается атомарно. Кеш игровых ресурсов проверяет фактический SHA-1. Запуск и переустановка экземпляра учитывают незакрытый журнал контента.

Файловая проверка относится к существующим предкам на момент проверки. Защита от одновременной замены каталогов и проверка на Windows остаются открытыми. Ранее созданные hardlink-размещения потребуют миграции. Размещение, которое recovery создало заново, записывается как copy. Совпадающий файл, найденный на месте при восстановлении, учитывается как внешний. Его способ размещения по содержимому определить нельзя.

Восстановление изолировано по экземплярам. Ядро читает журналы старого формата и записывает новые с версией 1. Если откатиться на старую версию ядра, она не сможет восстановить операцию, которую новая версия оставила незавершённой. Публичные методы ядра показывают незавершённые операции и допускают повтор после устранения конфликта. Интерфейс управления конфликтами ещё предстоит подключить.

Запуск по установленному профилю проверяет основной номер версии Java и известную архитектуру до файловых операций. Каталоги исключают недоступные релизы из подбора. Все запросы при составлении плана входят в общий лимит. Точный файл CurseForge подтверждается в пределах четырёх страниц списка с фильтром по загрузчику и версии Minecraft. Основание для фильтра описано в [REST-документации CurseForge](https://docs.curseforge.com/rest-api/#get-mod-files).
