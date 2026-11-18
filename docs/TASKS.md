# Реестр устранения замечаний

Основание: [аудит](core-architecture-audit-2026-11-17.md). Пакеты и критерии завершения: [план](README.md).

48 основных пунктов + 8 persistence + 17 локальных = **73 пункта**, все распределены. L01–L17 присвоены строкам таблицы раздела 5 аудита по порядку. Реализация началась в рабочей копии. Статус «запланировано» не означает подтверждённый runtime-баг: актуальность риска проверяется перед реализацией.

При закрытии заменить статус и записать commit, regression/contract test, результат проверки и ограничения. Для дефекта требуется regression; для проектного решения — реализованный проверяемый контракт и обоснование. Одного переноса файла или общей ссылки на сборку недостаточно.

| ID | Замечание | Пакет | Статус | Доказательство закрытия |
|---|---|---|---|---|
| A01 | P2 — `data` одновременно инфраструктура и сборщик приложения | P10 | Запланировано | — |
| A02 | P1/P2 — InstanceService зависит от внутреннего SQL контента | P02 | Исправлено | Общий InstanceActivity координирует launch, install, content и unregister. ContentService хранит узкие зависимости registry и activity. InstanceService использует контракт проверки публикации. Тест pending и unregister прошёл. Коммит `77bbbbe`. |
| A03 | P2 — Взаимная зависимость modloader и instance | P06 | Исправлено | Lifecycle установки принадлежит instance, транспорт находится в utils, правила манифеста в version. Аргументы запуска имеют собственную типизированную ошибку. Старый API jobs остался адаптером. Тесты резервации и ошибок аргументов прошли. Коммит `b2b7b0e`. |
| A04 | P2 — Общая политика Java принадлежит modloader | P07 | Исправлено | Java владеет политикой и выбором runtime. Адаптер версий разбирает поля манифеста. Выбор учитывает allow_newer и возвращает конкретный совместимый runtime. Старый импорт политики сохранён. Тест прошёл. Коммит `16e04df`. |
| A05 | P2 — SQL внутри content service | P03 | Исправлено | SQL и декодирование провайдера перенесены в storage::registered_mods. Коммит `0157666`. |
| A06 | P2 — Durable recovery DTO в API-модуле | P10 | Исправлено | Формат записи и PublishedFile перенесены во внутренний модуль journal. Коммит `ca2e8a9`. |
| A07 | P2 — Широкий pub API позволяет обходить service-инварианты | P11 | Запланировано | — |
| A08 | P2 — Три модели задач и частично дублированные состояния | P09 | Запланировано | — |
| A09 | P2 — Скрытая конфигурация и неверно определённая область root lock | P10 | Запланировано | — |
| B01 | P1 — Missing target под escaping symlink проходит child() | P01 | Исправлен обычный путь | Тест child_rejects_missing_targets_beneath_external_or_dangling_links. Конкурентная подмена предков и Windows требуют отдельной проверки. Коммит `7e72122`. |
| B02 | P1 — Hardlink делает CAS изменяемым из экземпляра | P01 | Частично | Новые установки используют копии. Зарегистрированные старые hardlink разъединяются на старте с сохранением байтов. Тест startup_detaches_registered_legacy_links_without_changing_bytes. Перемещённые или переименованные вне ядра файлы требуют сверки с диском. Коммит `171b9db`. |
| B03 | P1 — Pending content journal не блокирует обычный install | P02 | Исправлено | Тест unfinished_publication_blocks_launch_and_install_until_recovery. Общая проверка для launch/install выполняется при удержании резервации. Коммит `7e72122`. |
| B04 | P1 — Ошибка одного recovery блокирует остальные | P02 | Исправлено | Независимое восстановление экземпляров и фильтр по instance_id в SQL. Тест unreadable_journal_does_not_prevent_other_instances_from_recovering. Коммит `ca2e8a9`. |
| B05 | P1 — Нет прикладного способа разрешить RecoveryConflict | P02 | Частично | Ядро и CLI показывают незавершённые операции, позволяют повторить или отказаться с сохранением всех файлов. Тесты подтверждают сохранение файла, проверку владельца операции и освобождение leases. GUI остаётся открытым. Коммит `b804b63`. |
| B06 | P1 — Resolver не учитывает уже установленные моды | P03 | Частично | Граф учитывает известные активные моды, включая однозначно переименованные файлы. Тест проверяет точную уже установленную зависимость и конфликт в обе стороны. Для старых записей без метаданных нельзя проверить их собственные зависимости. Коммит `44c7dfe`. |
| B07 | P1/P2 — Ошибка невыбранного кандидата обрывает весь подбор | P04 | Исправлено | Непригодные релизы исключаются из подбора, ошибки сети сохраняются. Лимит 128 охватывает все запросы плана. Общий кеш точных ответов Modrinth хранит до 8 записей, включая 404. Тест повторного поиска без бюджета HTTP прошёл. Коммиты `e8171f4` и `c588ca4`. |
| B08 | P1 — CurseForge exact release не проверяет loader | P04 | Исправлено | Точный стабильный релиз ищется в списке CurseForge с фильтром по Minecraft и загрузчику. Проверяется до четырёх страниц. Локальный тест проверяет параметры запроса и отбор файлов. Живой API не проверялся. Коммит `e8171f4`. |
| B09 | P1 — Проверка Java перед launch закомментирована | P07 | Исправлено | Подготовка запуска проверяет major и известную архитектуру выбранной Java до файловых операций. Тест prepare_rejects_incompatible_java_before_creating_the_game_directory. Коммит `2e0221e`. |
| B10 | P1/P2 — Assets cache доверяет БД, не файлу | P01 | Исправлено | Тест asset_cache_checks_contents_even_when_database_already_has_the_asset. Коммит `7e72122`. |
| B11 | P1/P2 — Assets hash не валидируется перед slicing/path join | P01 | Исправлено | Тест invalid_hashes_fail_before_building_paths. Коммит `7e72122`. |
| B12 | P2 — Watcher/identity policy не подключены к рабочему индексатору | P03 | Исправлено | Для зарегистрированных модов работает полный скан перед запуском и фоновая сверка по событиям. Отсутствие watcher не отключает проверку запуска. Интеграционный тест с SQLite и системным уведомлением прошёл. Индексатор активен для 16 недавних экземпляров. Коммит `52633b0`. |
| B13 | P2 — Найденный existing file не получает provenance | P03 | Исправлено | Ядро регистрирует существующий JAR после сверки с хешем провайдера. Интеграционный тест подтверждает сохранение байтов и идентичности файла без сетевого скачивания. Коммит `ae4af77`. |
| B14 | P2 — Удалённый вручную мод продолжает блокировать reinstall | P03 | Исправлено | Удалённый файл перестаёт считаться установленным. Переустановка по тому же пути заменяет старую запись. Интеграционный тест прошёл. Коммит `0157666`. |
| B15 | P2 — Signature является слабым несогласованным snapshot | P03 | Исправлено | Снимок включает SHA-256 содержимого и проверку метаданных во время чтения. Проверяется до и после планирования и перед публикацией. Тест изменения байтов с прежним размером и mtime прошёл. Коммит `0157666`. |
| B16 | P2 — Atomic publish не означает durability при power loss | P08 | Контракт process-crash | API InstanceContentService и файловые helpers явно не обещают power-loss durability; порядок/ограничения описаны в publication-contract.md. Subprocess regression process_exit_recovers_each_publication_boundary_without_duplicate_links проверяет шесть FS/SQL границ на file SQLite без destructors; process_exit_followed_by_a_user_edit_keeps_the_journal_and_user_bytes сохраняет конфликтующий файл и leases. Полный test: 303 passed, 0 failed, 1 ignored; fmt/check прошли на Linux. Power-cut, падение внутри syscall/commit и Windows/macOS runtime не проверены; более сильная гарантия не заявляется и риск power loss не объявлен исправленным. Коммит `8752028`. |
| B17 | P2 — Recovery записывает запланированный placement вместо фактического | P02 | Исправлено | Созданный заново файл учитывается как copy. Совпадающий файл неизвестного происхождения учитывается как external. Коммит `ca2e8a9`. |
| D01 | P2 — Три streaming downloader | P06 | Исправлено | Assets, артефакты и Managed Java используют общий цикл HTTP, редиректов, чтения и хеширования. Адаптеры задают предел, алгоритм и политику HTTPS. Тесты лимита, перехода на HTTP, SHA-256, прогресса и ошибки записи прошли. Коммит `a94cba8`. |
| D02 | P2 — Пять похожих JSON/bytes transport | P06 | Исправлено | Общий HTTP-код проверяет статус, ограничивает тело и применяется к пяти путям метаданных. Лимиты и обработка 404 задаются адаптерами. Тесты размера, приоритета статуса, неверного URL и JSON прошли. Коммит `58e231f`. |
| D03 | P2 — Три cancellation type и утечка atomic representation | P06 | Исправлено | Один непрозрачный токен для Java, загрузки артефактов, контента и Forge. HTTP и ожидание повторной попытки прерываются сигналом. Тесты нескольких одновременных ожиданий, незавершённого чтения и сохранения вывода Forge прошли. Коммит `3444b6b`. |
| D04 | P1/P2 — Два launch pipeline и три rule evaluator | P07 | Исправлено | Обе схемы установок используют один pipeline запуска, общий evaluator правил и одинаковую обработку аргументов и cwd. После проверки импортов ядра и GUI дублирующий launch builder удалён. Сравнительный тест старой и новой установки прошёл. Коммиты `03ed44b` и `3ea6a67`. |
| D05 | P2 — Hash loops повторяются, контент читается многократно | P06 | Исправлено | Общий проход читает файл частями с отменой. CAS проверяет три хеша при копировании и валидирует staging перед публикацией без повторного чтения файла. Forge по-прежнему считает только SHA-1. Тест проверки staged-байтов и отсутствия публикации при отказе прошёл. Коммит `001025f`. |
| D06 | P2 — Copy Maven tree почти одинаков в Forge/Cleanroom | P06 | Исправлено | Forge и Cleanroom вызывают один перенос дерева Maven с отменой и атомарной заменой файлов. Символические ссылки и специальные файлы отклоняются. Тесты замены, отмены и ссылок прошли. Коммит `4d037f1`. |
| D07 | P2 — Несколько stores без единого контракта идентичности | P08 | Частично | `c20d4a9`: контент/runtime используют utils::object_store, SHA-256 bytes identity, verified SHA-1 hints и copy placements; `8a38e9d`/`9659851`: общий Collection и runtime blob GC. `081230b`: assets используют тот же immutable store, Mojang SHA-1 layout — изолированные placements; `534b79d`: отдельный asset CAS collector с complete owners/SQL/index/placement roots. SQL identity сохраняется, corrupt CAS не перезаписывается. Полный Linux fmt/check/test/build: 445 passed, 0 failed, 1 ignored. Managed Java ещё открыт; asset SHA-1/virtual projections не удаляются и требуют отдельной ownership migration для более агрессивного GC. |
| D08 | P2 — Нет GC поколений/CAS, sweeper не понимает новые partials | P08 | Частично | Scratch: `31d9d2f`/`7859409`/`9be8fe8`/`0493299`; admission: `2d37a3d`; content CAS GC: `aef7a11`; runtime pins/catalogue/generation replay: `dd96cdd`/`47dbe51`/`150e1f2`; общий Collection/blob GC: `8a38e9d`/`9659851`; asset CAS GC: `534b79d`; managed Java owned staging/recovery: `b5bc1e7`. Managed recovery требует dead versioned operation owner и совпадающий bounded regular marker; shared writers/exclusive recovery плюс per-operation lock. Drop caller сохраняет guard start/install/validation/recovery workers. Полный Linux fmt/check/test/build: 461 passed, 0 failed, 1 ignored; 16 новых managed tests, four process-exit boundaries и отдельный live writer process. Managed Java published homes/CAS ещё не собираются; SHA-1 asset placements не собираются. Реальные JVM/descendants, power-cut и Windows/macOS не проверены. Подробности — retention-contract.md. |
| D09 | P2 — Одинаковые settings/rename меняют разные события | P05 | Исправлено | Оба сервисных пути публикуют одинаковые доменные события. Старые уведомления Scan и Option оставлены для совместимости. Тест событий патча прошёл. Коммит `7e0e7d1`. |
| D10 | P2 — Platform normalization дублируется и расходится | P06 | Исправлено | Единый HostPlatform задаёт архитектуру и версию ОС. Установка и запуск получают один RuleContext. Названия Prism и Java остаются в своих адаптерах. Тест общего контекста и алиасов i386/i586 прошёл на Linux. Коммит `f938656`. |
| Q01 | P2 — Overview patch частично сохраняется при ошибке rename | P05 | Исправлено | Имя и поля обзора меняются в одной транзакции. Тест конфликта имени проверяет откат тегов. Коммит `d2f869c`. |
| Q02 | P1/P2 — Нет валидации памяти; JVM quoting ломается | P05 | Исправлено | При записи и запуске проверяется эффективный диапазон памяти. Аргументы разбираются по правилам shell quoting без запуска shell. Тест наследования и аргумента с пробелом прошёл. Коммит `2be46aa`. |
| Q03 | P2 — Registry mutex не обеспечивает согласованность cache | P05 | Исправлено | Кеши экземпляров и Java синхронизированы. Java перечитывает обновлённые записи после backfill и при list. Тест backfill прошёл. Коммит `d16c640`. |
| Q04 | P2 — Managed Java зависит обратно от owning JavaService | P09 | Запланировано | — |
| Q05 | P2 — Один плохой managed record ломает initialize всех | P09 | Запланировано | — |
| Q06 | P2 — Forge processors выбирают просто самый новый Java major | P07 | Исправлено | Минимум Java определяется по базовым классам клиентских процессоров и их classpath. Выбирается ближайший подходящий major. Тест зависимостей, multi-release и недоступной Java прошёл. Тестового запуска внешнего Forge installer не было. Коммит `d0a7933`. |
| Q07 | P2 — Typed causes превращаются в строки и UI-текст | P10 | Запланировано | — |
| Q08 | P2 — Retry/отмена неодинаковы и имеют лишние ожидания | P06 | Исправлено | Потоковые загрузки повторяют только временные сбои. Retry-After учитывается до 120 секунд. После последней попытки ожидания нет. Отмена передана в assets и проверку файлов. Тесты 404, отказа доступа, исчерпания диска, Retry-After и сигнала отмены прошли. Коммит `b8bdd5a`. |
| Q09 | P2 — Cache-hit copy не атомарна | P01 | Исправлено | Cache-hit публикует копию через временный соседний файл. Тест cancelled_cache_hit_preserves_existing_destination. Коммит `7e72122`. |
| Q10 | P2/P3 — ScanIntegrity сообщает фиктивное значение | P05 | Исправлено | Проекция проверяет каталог экземпляра перед заполнением пути. Отсутствующий или непроверенный путь остаётся пустым. Тест прошёл. Коммит `7e0e7d1`. |
| Q11 | P2/P3 — Новый mods scanner дублирует inventory и паникует | P07 | Исправлено | Запрос модов использует общий файловый индекс и возвращает список или типизированную ошибку. Тест отсутствующего каталога, фильтра архивов и ошибочного пути прошёл. Коммит `45f0497`. |
| Q12 | P2 — Background tasks не имеют общего supervision/shutdown | P09 | Запланировано | — |
| S01 | P2 — Journal не версионирован | P02 | Исправлено | Новые записи содержат версию 1. Ранее сохранённые массивы читаются. Неизвестная версия возвращает отдельную ошибку. Коммит `ca2e8a9`. |
| S02 | P2 — Stringly typed состояния | P02 | Исправлено | Состояния и переходы журнала ограничены схемой и Rust enum. Провайдер, размещение и статусы сверки передаются как типизированные значения. Старые журналы читаются. Коммит `2a0b147`. |
| S03 | P2 — Preferred provider FK не гарантирует принадлежность object | P08 | Исправлено | Миграция 0011 проверяет принадлежность при INSERT и UPDATE с обеих сторон. Старые неверные preferences сбрасываются без изменения файлов, object_id и журналов. Тест provider_ownership_upgrade_preserves_files_and_enforces_all_mutations проверяет upgrade, все пути нарушения, SET NULL, CASCADE и RESTRICT на SQLx pool. Regression и cargo check прошли. Коммит `e5715cf`. |
| S04 | P3 — plan_json содержит фиктивный `{}` | P02 | Исправлено | Миграция 0009 удаляет дублирующий plan_json. Версионированный recovery_json остаётся источником плана. Интеграционные тесты записи и восстановления прошли. Коммит `b804b63`. |
| S05 | P2 — Leases покрывают только publishing | P08 | Частично | `0e97c9a`: preparing до lookup/download и atomic identity/lease; `2d37a3d`/`aef7a11`: content admission/owner DB/roots/SQL locks; `dd96cdd`: game/direct-child pins до reads/outputs; `47dbe51`/`150e1f2`/`9659851`: runtime roots/crash replay; `081230b`/`534b79d`: asset workers/complete roots/owned collector. Drop caller не освобождает worker SQL/admission; kill-on-drop не подтверждает exit. Minecraft читает asset copies, collector не удаляет их: отдельный game CAS pin не требуется для текущего asset GC. Полный Linux fmt/check/test/build: 445 passed, 0 failed, 1 ignored. Managed Java pins и реальные JVM/descendants открыты. |
| S06 | P2/P3 — Абсолютные paths и дублируемая истина | P08 | Частично | `c20d4a9`: CAS paths из root/SHA-256, 0013 запрещает store_path, сохраняет FK/leases; `a69c75a`: root-aware upgrade/writers обоих legacy library snapshots и linked_libs_at; `2c38a4b`: managed Java SQL pair upgrade и root-aware DbJavaStore/JavaService. Меняются только доказанно внутренние placements; unknown/external/old-root/unsafe/conflicting paths сохраняются без guessing/merging IDs. Runtime consumers получают absolute paths текущего root; unrooted reader relative managed rows отвергает. Полный Linux fmt/check/test/build: 474 passed, 0 failed, 1 ignored; 13 новых Java path tests, включая relocation, IDs/metadata/FK/identity, SQL crash boundaries, aliases/links, bulk dedup/early rejection, non-Unicode, caller drop. Historical catalogue DB paths автоматически не переносятся; внешние Java paths не становятся managed, поэтому полный S06 остаётся частичным. |
| S07 | P2 — Hash не определяет semantic library identity | P08 | Исправлено | Миграция 0012 сохраняет старые пути, роли и порядок в instance_library_placements. Новые snapshots принадлежат экземпляру и не меняют чужие записи; download/cache key учитывает путь. Тесты identical_bytes_keep_distinct_instance_paths_roles_and_positions, missing_destination_is_not_a_cache_hit_for_identical_bytes_elsewhere и library_placement_upgrade_preserves_legacy_order_paths_and_roles прошли. Полный test: 288 passed, 0 failed, 1 ignored; cargo check прошёл. Старые tables сохранены как upgrade snapshot; ранее потерянные роли восстановить из БД нельзя. Коммит `b623af7`. |
| S08 | P2/P3 — NameTaken обрабатывается неодинаково | P05 | Исправлено | Создание, изменение имени и патч обзора возвращают типизированный конфликт при нарушении уникальности. Проверено на SQLite. Коммит `d2f869c`. |
| L01 | P2 — ZIP extraction блокирует executor; ошибки архивов только логируются, возвращается Ok. Общий extractor в blocking worker, ошибки наружу. Старый fallback реально вызывает этот путь. | P07 | Исправлено | Обе точки вызова используют одну распаковку в blocking worker. Ошибка архива прерывает подготовку запуска. Тест повреждённого ZIP прошёл. Коммит `afd7d64`. |
| L02 | P2 — Каждый запуск создаёт UUID natives directory без cleanup. Владение каталогом связать с процессом, удалять после exit и stale leftovers при старте. | P07 | Исправлено с ограничениями | Каталог принадлежит подготовке и затем процессу; удаляется после подтверждённого выхода. Startup sweep удаляет только каталоги с подтверждённо отсутствующим владельцем и, после spawn intent, отсутствующим child PID. Тест stale_sweep_keeps_live_launches_and_unverified_ownership сохраняет живые и неизвестные каталоги. При неполной записи PID и его повторном использовании очистка консервативно откладывается; старые каталоги без markers не удаляются. Коммиты `0ecd437`, `db4de73`. Windows/macOS runtime не проверен. |
| L03 | P2 — Лексический starts_with принимает libraries/../outside. Проверять относительный остаток/предков. Это validation outputs, не sandbox исполняемых Java processors: они и так имеют права launcher. | P01 | Исправлено | Forge проверяет относительный остаток через files::child. Тесты файлового helper. Коммит `7e72122`. |
| L04 | P2 — Cache key — basename URL: разные origin/path с profile.json сталкиваются. Запись неатомарна. Использовать URL identity и общий repository cache. | P06 | Исправлено | Манифесты и загрузчики используют общий SHA-256 ключ полного URL и атомарную запись. Тест двух origin, разных путей и параметров прошёл. Старый кеш по basename требует однократного сетевого обновления. Коммит `998c869`. |
| L05 | P2 — Read-modify-write JSON registry без serialization теряет параллельные регистрации. Atomic persist не защищает от lost update. Нужен mutex/transaction. | P10 | Запланировано | — |
| L06 | P2 — Очищается память, но не disk/in-flight refresh; следующий read может вернуть прежнюю запись. Уточнить контракт/название, для force invalidation — generation token. | P10 | Запланировано | — |
| L07 | P3 — Eager unwrap_or ищет default home даже при root_override и может ошибиться. Явный match с lazy async fallback. | P10 | Запланировано | — |
| L08 | P3 — Нулевые capacity/buffer_size не запрещены: panic channel либо ложный EOF. NonZeroUsize/Result и разумные пределы. | P01 | Исправлено | Допустимо до 4096 буферов и 64 МиБ, умножение проверяется до выделения памяти. Граничный тест прошёл. Коммит `9936d17`. |
| L09 | P3 — Usage create требует лишний loader, dispatch его не принимает. Исправить help; parse/help выполнять до GlobalState, иначе даже ошибочная команда запускает DB/recovery. | P10 | Запланировано | — |
| L10 | P3 — Эвристический parser и строковые paths рядом с проверяемым MavenCoordinate. Проверить потребителей, deprecated/migrate/delete; форматы результата различаются, механическая замена неверна. | P07 | Исправлено | Удалён неиспользуемый эвристический parser. Дескрипторы проходят проверку относительных путей. Для старого публичного строкового helper отмечена устаревшая совместимость. Тест пути и выхода из корня прошёл. Коммит `a5af6ad`. |
| L11 | P3 — Read не обновляет LinkedHashMap order: ограниченный cache фактически FIFO, не LRU. Документировать или изменить при измеренной необходимости. | P05 | Исправлено | Чтение обновляет порядок кеша Java. Граничный тест LRU прошёл. Коммит `d16c640`. |
| L12 | P3 — n-1 при n=0 может panic/underflow. Zero-based API либо checked_sub/NonZeroUsize. | P06 | Исправлено | Порядковый номер 0 возвращает None. Нумерация с 1 и байтовые позиции UTF-8 сохранены. Граничный тест прошёл. Коммит `aedc5f9`. |
| L13 | P3 — Active jobs и история имеют общий лимит; error предлагает restart. Bounded completed ring отдельно от active capacity, периодический pruning plans. | P09 | Запланировано | — |
| L14 | P3 — TTL истории от начала задачи, не окончания: долгая установка может сразу исчезнуть при новом start. finished_at отдельно. | P09 | Запланировано | — |
| L15 | P2/P3 — Ошибка одного major скрывает успешно полученные пакеты всех остальных. Определить контракт partial results+warnings либо явную строгую атомарность. | P04 | Исправлено | Каталог сохраняет успешно полученные пакеты и возвращает предупреждения по major. Полный отказ остаётся ошибкой. Тест прошёл. Коммит `bba652e`. |
| L16 | P3 — IDs непустые/уникальные, но hash и URL segment не проверяются. Общий URL builder и digest type. | P06 | Исправлено | Индекс Prism проверяет типизированный SHA-256 и идентификатор версии. URL строится общим builder с кодированием сегмента. Тест произвольных символов и некорректного digest прошёл. Коммит `61a12f8`. |
| L17 | P2/P3 — Finish публикуется до удаления active snapshot; terminal result не сохраняется. Для переподписки/overflow нужны reconciliation и bounded terminal history. | P09 | Запланировано | — |

Дополнительные обязательные работы P11: consumer imports, удаление неподключённого кода и итоговая Linux/Windows/macOS проверка. Сборочная среда P00 отслеживается отдельно в плане.

### Продолжение P08 после runtime pins

`47dbe51` уточняет частичные D08/S05: library catalogue/current roots gate теперь
реализован (generation/blob deletion protocol ещё открыт). Attach предшествует
workspace outputs/launch reads, unknown/historical/ephemeral owners fail-closed,
known writer допускается под shared lock без ожидания чужой установки. Все
catalogued persistent SQLite identities удерживают BEGIN IMMEDIATE и maintenance;
snapshot учитывает текущие профили и оба legacy placement snapshots, missing
artifacts и SHA-1 независимо от ролей. Unknown schema/fields/paths/hashes,
missing/replaced owners и broad game pins блокируют gate, aliases — не roots.
Owned attach переживает drop caller. Полный Linux fmt/check/test/build:
**383 passed, 0 failed, 1 ignored**, 13 новых tests и прежние content GC regressions.
На этой исторической точке (`47dbe51`) startup был diagnostic-only; последующие
commits ниже включают проверенные collectors. Подробности и логи —
[retention contract](retention-contract.md#runtime-catalogue-и-current-roots-gate).

`150e1f2` продолжает D08/S05: versioned generation record до outputs, отказ
uncatalogued writer и no-clobber UUID directory; startup GC только catalogued,
dead-owned, unrooted поколений старше 7 дней и без live/unknown process pins.
Intent/quarantine/outputs/metadata/completion replay сохраняет неизвестные данные,
links и replacement identity; новые current roots до replay возвращают оставшиеся
quarantined bytes без overwrite. Весь collector удерживает SQL/maintenance после
drop caller. Полный Linux fmt/check/test/build: **399 passed, 0 failed, 1 ignored**,
16 новых tests, включая subprocess exits шести deletion границ и проверку возраста
новых outputs. Runtime blob GC, portability, assets и managed Java остаются
открытыми; P08 ещё не объявлен завершённым.

`8a38e9d` выделяет общий FS deletion protocol в нейтральный Collection, без SQL
и domain roots; полный Linux fmt/check/test/build: 399 passed, 0 failed, 1 ignored.
`9659851` подключает startup runtime blob GC после generation maintenance (7 дней).
Roots — SHA-1 current profiles/обоих legacy snapshots и проверенные maven/work bytes
всех surviving generations. Library → blobs admission и все DB write-locks живут
в owned collector до cleanup; live/unknown/future preparations, unknown layouts,
pending/corrupt quarantine откладывают всю blob-область, даже ещё не materialized
inputs. Aliases/URL indexes не semantic roots, flat SHA-1 legacy blobs сохраняются.
Полный Linux fmt/check/test/build: **417 passed, 0 failed, 1 ignored**; 18 новых
blob tests: четыре process-exit границы, новые roots/reimport age при replay,
drop caller на snapshot и в blocking intent, independent blob reader, несколько DB,
missing files, surviving generations, future timestamps, canonical root aliases,
unknown metadata/layout/quarantine, links и startup generation→blob integration.
P08 остаётся частичным: portability legacy paths, assets и managed Java открыты.

`a69c75a` продолжает S06: owned migration под library maintenance/BEGIN IMMEDIATE
меняет только paths обоих legacy snapshots, без изменения bytes, roles, positions,
associations, InstalledProfile JSON и retention identity. Не угадывает прежний root
после relocation. Старый linked_libs deprecated: это raw snapshot, paths могут быть
relative; production legacy launch использует root-aware linked_libs_at под ранее
добавленным broad pin. Root-aware writers отвергают outside до transport и сохраняют
relative placements. Семь новых regressions и полный Linux fmt/check/test/build:
**424 passed, 0 failed, 1 ignored**, логи core-library-paths-verified-{check,test,build}.
Assets/managed Java ещё открыты; missing historical DB остаётся fail-closed, его
catalogue path автоматически не переносится. Windows/macOS/manual не проверены.

`081230b` продолжает D07: assets теперь хранят immutable SHA-256 bytes в общем
ObjectStore (`assets/objects/.sonata-cas`), Mojang SHA-1 paths остаются проверяемыми
copy placements. Legacy valid bytes импортируются без сети/удаления/смены SQL IDs;
missing/corrupt placement восстанавливается только из независимо verified CAS.
Corrupt authoritative object, symlinks/escaping ancestors не перезаписываются;
register требует verified bytes до SQL. Owned blocking cache/import worker держит
admission после drop caller. Startup очищает только dead-owned CAS scratch.
Полный Linux fmt/check/test/build: **433 passed, 0 failed, 1 ignored**; девять новых
tests плюс два прежних asset regressions: relocation, copies, SQL/alias/corrupt
bytes, cancellation, links, drop caller, process exit между import и SQL/replay,
production sync без transport. Логи core-assets-cas-verified-{check,test,build}.
Published asset GC намеренно пока не включён: roots/complete catalogue/pins gate
ещё не доказаны; SQL rows и aliases сами по себе не разрешают удаление.

На следующей точке `534b79d` включён только private asset CAS GC (7 дней), не
Mojang placements/virtual projections. Каждый writer attach до bytes; старые
uncatalogued stores и unknown/ephemeral/missing/replaced owner DB блокируют GC.
Conservative roots: все asset SQL hashes всех owners, hashes всех regular indexes
и всех canonical SHA-1 placements, даже corrupt placements (для будущего repair).
Unknown/future index/object/layout metadata, links и pending imports блокируют
область. Owned collector удерживает CAS admission и все SQL write-locks до FS
completion после drop caller. Повторяемый Collection не меняет SQL identity.
Полный Linux fmt/check/test/build: **445 passed, 0 failed, 1 ignored**; 12 новых
tests: multiple DB/identity, unknown ownership, SQL/index/placement roots, layouts,
age/future bytes, four process-exit boundaries, replay new roots/reimport age,
corrupt intents, drop caller snapshot/blocking intent с SQL probe, canonical aliases
и links, startup integration. Первый focused run обнаружил пропущенный test hook
после intent cleanup; hook добавлен, expanded и полный повторные прогоны прошли.
Логи core-assets-gc-verified-{check,test,build}; managed Java остаётся следующим P08.

`b5bc1e7` продолжает managed часть D08/S05: versioned owner (UUID/PID) в record
и staging marker; no-clobber staging creation, cleanup только matching regular
bounded marker. Recovery под exclusive canonical .managed admission; normal
writers под shared admission и exclusive per-operation scope. Dead-owned scratch
terminal records тоже очищается, опубликованные homes не удаляются. Active
live/unknown/legacy/future owner и unknown/mismatched marker сохраняются без
перезаписи record. Foreign cancel запрещён; validation перечитывает durable record
под operation lock, поэтому stale snapshot не присваивает чужую active операцию.
Paths records/staging/home/executable проверяются до чтения/записи/probe;
существующий publication destination не заменяется (внешняя concurrent FS подмена
не поддерживается). Drop start/validation/recovery caller не освобождает worker
admission во время blocking staging/cleanup/record commit.
Полный Linux fmt/check/test/build: **461 passed, 0 failed, 1 ignored**; 16 новых
tests плюс прежние managed regressions. Four process-exit boundaries (directory
creation до marker, marker, home publication, terminal staging cleanup), отдельный
live writer process, canonical aliases/escaping links, unknown owners/markers,
durable identity/home safety, duplicate operation, stale validation, caller drop
и preexisting destination. Focused: 41 managed tests. Первый expanded run не
скомпилировал visibility test hooks; scope исправлен, expanded/focused/full прошли.
Логи core-managed-owner-verified-{check,test,build}; прежние warnings/ignore.
Q04 registrar/Q05 corrupt-record isolation остаются P09; managed archive identity,
SQL portability и published-home roots/process pins/GC остаются P08, не PASS.

`2c38a4b` продолжает S06: JavaService использует root-aware DbJavaStore; новые
managed SQL exec/home pairs сохраняются relative под managed/ текущего Java root.
Consumer JavaRuntime возвращает resolved absolute placements. External/manual/
discovered rows не мигрируются; old-root/outside/unsafe/mixed/conflicting pairs
сохраняются, IDs/FK никогда не сливаются. Upgrade под тем же exclusive .managed
admission и BEGIN IMMEDIATE; owned task удерживает SQL/lock после drop caller.
Startup пытается upgrade без discovery/probe; busy/error откладывается, следующий
service initialize/discovery повторяет. Уже loaded registry snapshots не pins.
13 новых path tests: metadata/IDs/FK/DB identity/bytes, relocation, missing/external/
old paths, aliases/escaping ancestors, conflicts/bulk dedup, unsafe/non-Unicode
early rejection и bulk rollback, root-aware read, caller drop с SQL probe, busy
retry, production service и actual process exit до/после SQL commit.
Полный Linux fmt/check/test/build: **474 passed, 0 failed, 1 ignored**, focused
Java: 83 passed, 1 ignored. Логи core-managed-paths-verified-{check,test,build}.
Managed archive identity и published-home roots/pins/GC остаются следующим P08;
catalogued DB paths не переносятся, реальные JVM/platform/manual ещё не проверены.
