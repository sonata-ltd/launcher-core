# Верификация и исправления Launcher Core

Исходный HEAD `bc1945d8cf4d0234f1f0d77ab3ab1ea6939bb40c`. Источник гипотез — `/tmp/launcher-core-readonly.vnN6DeRh/launcher-core-audit.md`. Локальные протоколы проверки — `/tmp/launcher-remediation.pj8qXHOL`.

До изменений Git показал ветку `refactor/all-audit-findings`, две изменённые tracked-записи `.cargo/config.toml` и `Cargo.lock`, untracked `not-a-database-url` и пустой staged diff. Исходные SHA-256 всех tracked-файлов, отдельные хеши трёх пользовательских файлов и исходный diff сохранены в каталоге протоколов.

## Исходная проверка

Linux x86-64, Rust 1.98.1. Штатный runner использовал `--locked --offline` и отдельные временные каталоги. `fmt`, `check --all-targets`, `build` и `clippy --all-targets` прошли. Первый `test` под sandbox дал 16 ошибок локального bind. В прогоне с разрешённым 127.0.0.1 прошли 620 библиотечных тестов, 2 CLI, 2 интеграционных и 18 doctests. Один тест обнаружения установленной Java остался ignored. Python runner прошёл 8 тестов. Windows GNU прошёл `check`, линковку тестов и `build`. Нативного выполнения на Windows и macOS не было.

## Журнал

| ID | Группа | Вердикт | Состояние |
|---|---:|---|---|
| F01 | 1 | CONFIRMED | FIXED / VERIFIED |
| F09 | 1 | CONFIRMED | FIXED / VERIFIED |
| F22 | 1 | CONFIRMED | FIXED / PLATFORM VALIDATION PENDING |
| F06 | 2 | CONFIRMED | FIXED / VERIFIED |
| F13 | 2 | CONFIRMED | FIXED / VERIFIED |
| F15 | 2 | CONFIRMED | FIXED / PLATFORM VALIDATION PENDING |
| F20 | 2 | CONFIRMED | FIXED / VERIFIED |
| F23 | 2 | CONFIRMED | FIXED / VERIFIED |
| F02 | 3 | CONFIRMED | BLOCKED |
| F03 | 3 | CONFIRMED | FIXED / VERIFIED |
| F04 | 3 | CONFIRMED | FIXED / PLATFORM VALIDATION PENDING |
| F07 | 3 | CONFIRMED | FIXED / PLATFORM VALIDATION PENDING |
| F14 | 3 | PARTIALLY CONFIRMED | FIXED / PLATFORM VALIDATION PENDING |
| F05 | 4 | CONFIRMED | FIXED / VERIFIED |
| F08 | 4 | CONFIRMED | FIXED / VERIFIED |
| F17 | 4 | CONFIRMED | FIXED / VERIFIED |
| F19 | 4 | CONFIRMED | FIXED / VERIFIED |
| F10 | 5 | CONFIRMED | FIXED / VERIFIED |
| F11 | 5 | PARTIALLY CONFIRMED | FIXED / VERIFIED |
| F12 | 5 | CONFIRMED | FIXED / PLATFORM VALIDATION PENDING |
| F16 | 5 | PARTIALLY CONFIRMED | FIXED / VERIFIED |
| F18 | 5 | PARTIALLY CONFIRMED | FIXED / VERIFIED |
| F21 | 5 | CONFIRMED | FIXED / PLATFORM VALIDATION PENDING |

Из 23 находок подтверждено 19, частично подтверждено 4. Вердикты FALSE POSITIVE,
OBSOLETE и NOT VERIFIED не присваивались. Состояние и вердикт отражают разные
результаты проверки. Для F11, F16 и F18 состояние относится к подтверждённой части.

## Группа 1. Asset index и managed Java

- **F01 — CONFIRMED, `d9131ac`.** До изменений реальный `init::install::execute` вернул профиль, `launch::resolved::prepare` передал ID, затем чтение `indexes/<id>.json` вернуло `NotFound`. В `download/assets/index.rs` добавлены проверка длины и SHA-1 сырых байтов, отдельный URL-кэш под `meta/asset-indexes` и атомарная публикация по игровому ID без перезаписи. `init/install.rs` публикует индекс после проверки артефактов и до записи профиля. Новый профиль проверяет опубликованный индекс при подготовке. Конфликт ID и байтов требует разрешения оператором, текущий индекс сохраняется. Исторические профили без признака верификации продолжают читаться. Контракт legacy-проекций `virtual` и `resources` остался отдельным релизным ограничением. `asset_index_install_publication_matches_prepared_game_id_after_restart` проверяет новый файловый SQLite после переоткрытия и отклонение ошибок SHA-1, размера и конфликта ID.
- **F09 — CONFIRMED, `d9131ac`.** До изменений ответ с двумя `objects` завершил установку успешно. Установка и GC используют один декодер первичного JSON; проверяются дубли корневых полей, имён объектов и полей в объекте. `asset_index_install_rejects_duplicate_keys_before_persistence` доказывает отказ до записи профиля и индекса. Несовместимая будущая схема отклоняется с диагностикой. GC по-прежнему консервативно удерживает старые неоднозначные индексы.
- **F22 — CONFIRMED по Windows semantics, `b33ceed`.** `java/paths.rs` кодирует компоненты через `/`. Записи операции и маркеры home, регистрация, SQL upgrade и retention учитывают один формат. Исторический Windows operation path допускается при совпадении с собственной generation. Тесты `nested_managed_home_uses_portable_paths_through_publication_and_recovery` и `managed_sql_encodes_nested_native_paths_as_portable_relative_paths` прошли на Linux и слинкованы для Windows GNU. Требуется native Windows, включая проверку регистрации JRE с реальным исполняемым файлом и Windows-исторической SQLite. Linux не трактует обратную косую черту как разделитель.

Прогон `group1-linux-retry` прошёл полностью. 624 библиотечных, 2 CLI, 2 интеграционных и 18 doctests, один ignored. Первый групповой прогон выявил конфликт маршрутов в HTTP-фикстуре F09. Фикстура получила раздельные URL, assertions остались прежними. Логи `assets-before.log`, `group1-linux-retry`, `group1-clippy.log` и `group1-windows-gnu` доступны в каталоге протоколов.

## Группа 2. Владение и shutdown

- **F06, F13, F15, F23 — CONFIRMED.** Четыре новых теста `ownership_*` через публичный API упали на исходном поведении и прошли после изменения. При F06 дочерний процесс открывал ту же SQLite через `dirs.cache_db`, пока родитель удерживал её через `database_url`. Постоянная привязка к layout и lifetime locks добавлены в `data/ownership.rs`, `data/db/mod.rs` и bootstrap. Старый файл SQLite получает привязку при первом успешном открытии новой версией. Автоматический перенос существующего layout между путями требует отдельной гарантии происхождения данных.
- **F20 — CONFIRMED.** В `bootstrap-before-retry.log` зафиксирован успешный повторный захват store lock после drop bootstrap в точке FS success → SQL completion. В `bootstrap-after.log` тот же тест подтверждает сохранение staging, блокировки и завершение SQL. Owned startup и отдельные recovery workers удерживают guards до завершения.

Коммит группы `a8265a6`. В `group2-linux` прошли 625 библиотечных, 2 CLI, 6 интеграционных и 18 doctests. `group2-windows-gnu` подтвердил компиляцию и линковку. Clippy завершился с предупреждениями.

- **F13.** `InstanceService`, `OptionsService` и `JavaService` используют общий admission. Java auto-select после публикации профиля выдаёт отдельную диагностику. `ownership_shutdown_rejects_instance_and_options_mutators` проверяет create, rename, settings, unregister и сохранение строки. Вызовы управления уже допущенными операциями по-прежнему могут послать сигнал остановки.
- **F15.** `LayoutLease` захватывает общую launcher/instances блокировку один раз при совпадении каталога. `ownership_coincident_launcher_and_instance_roots_open_once` прошёл на Linux. Нужен native NTFS/APFS с алиасами.
- **F23.** DB-backed сервисы удерживают полномочие layout через общий `Database`. Game watcher сохраняет lease до завершения обработки процесса. `ownership_exported_instance_service_keeps_lifetime_locks` подтверждает недоступность повторного bootstrap до освобождения экспортированного сервиса.

## Группа 3. CAS и файловая идентичность

- **F02 — CONFIRMED, частичное исправление `8795203`.** Добавлена проверка границ путей assets и Forge. Linux-регрессия `initialization_refuses_an_assets_objects_directory_redirect` прошла. Windows-фикстура слинкована. Общая гарантия при конкурентных изменениях дерева требует операций от удерживаемого каталога на каждой целевой ОС.
- **F03 — CONFIRMED, `5d2cd98`.** `modified_cas_bytes_are_never_published_under_the_registered_digest` меняет подконтрольный источнику файл после его исходной проверки и до копирования. До исправления SQL зарегистрировала старый digest для других байтов. `content/install.rs` проверяет digest и размер при staging, проверяет SHA-256 перед rename и освобождает preparation при отказе. Кооперативные процессы блокируются admission. Подмена файлов внешним writer после последней проверки остаётся областью F02.
- **F04 — CONFIRMED, `43761e9`.** `legacy.rs` отделяет неизвестную identity от несовпадения и повторно проверяет физическую identity перед заменой. Linux-регрессии `unknown_candidate_identity_defers_even_when_one_legacy_alias_is_identified` и `absent_cas_cannot_prove_ownership_of_a_recorded_path` прошли. Нужна нативная проверка Win32 file ID и sharing. Копирование сохраняет обычные permissions. Сохранение ACL и xattrs остаётся неподтверждённым.
- **F07 — CONFIRMED.** `sql_failure_after_isolation_preserves_bytes_cas_and_original_binding` подтверждает FS success → SQL failure. Для восстановления требуется долговременное подтверждение опубликованной копии. Решение описано в продолжении ниже.
- **F14 — PARTIALLY CONFIRMED.** SQL сравнивал точное представление пути, тогда как эквивалентность зависит от тома. Для неоднозначных исторических записей добавлен безопасный отказ. Нативные проверки NTFS и APFS остаются открытыми.

Протоколы `group3-before-retry.log`, `group3-focused-retry.log`, `group3-linux`, `group3-windows-gnu` и `group3-clippy.log`. На Linux прошли 628 библиотечных, 2 CLI, 6 интеграционных и 18 doctests, один ignored. Windows GNU подтвердил компиляцию и линковку.

## Группа 4. План и текущее состояние

- **F05, F08, F17, F19 — CONFIRMED.** Все четыре теста в `group4-before.log` получили заявленные сбои на исходных реализациях. В `group4-focused-retry.log` прошли 19 проверок. F05 добавляет упорядоченный fingerprint SQL-фактов и типизированное устаревание preview после adoption. F08 сопоставляет exact bindings до relocation и оставляет конкурирующие привязки неподтверждёнными. F17 потребляет preview под тем же mutex после admission. F19 использует условный SQL UPDATE и отправляет событие только после фактического изменения. Файлы `content/storage.rs`, `content/service.rs`, `instance/service.rs` и `instance/settings`.

Коммиты `75d80ee` (F05, F08, F17) и `eb1a2bb` (F19). Тесты `sql_only_adoption_invalidates_a_second_preview_without_touching_file_bytes`, `exact_binding_reserves_a_file_against_another_historical_seed`, `a_busy_instance_keeps_its_previously_built_preview_for_one_retry` и `java_auto_selection_preserves_a_manual_choice_made_before_update`. Полный Linux-прогон прошёл с 632 библиотечными тестами. Windows GNU подтвердил компиляцию и линковку.

## Группа 5. Надёжность и ограничения ресурсов

- **F10 — CONFIRMED, `2002dc6`.** Recovery и retention в `java/managed` получили общий строгий декодер. Тест `duplicate_managed_record_keys_defer_recovery_and_collection` прошёл после исходного отказа проверки. Запись сохраняется и появляется в диагностике, сборка откладывается.
- **F11 — PARTIALLY CONFIRMED, первое исправление `7accf62`.** В метаданных проверяется схема URL, сохраняются типизированные транспортные ошибки, диагностический URL ограничен. Политика источников и перенаправлений описана в продолжении ниже.
- **F12 — CONFIRMED, `7accf62` и `d6d3fd9`.** `metadata_cache` ограничивает чтение с диска. Cleanroom ограничивает размер метаданных пакета и совокупных patches. ZIP-распаковка ограничивает фактические байты, managed Java также ограничивает число записей. `forge` пишет объединённый лог до 32 МиБ и продолжает читать вывод после исчерпания лимита. Прошли `oversized_cached_metadata_is_refused_before_it_reaches_the_repository_caller`, `zip_entry_budget_is_enforced_before_creating_outputs`, `processor_log_limit_drains_output_without_blocking_child_completion` и `the_platform_pipe_finishes_and_keeps_one_truncation_notice`. Последний тест ожидает нативного Windows/macOS прогона.
- **F16 — PARTIALLY CONFIRMED, `75d80ee` и `37020a0`.** Обратный индекс digest устраняет повторный обход архива для каждой записи planner reconciliation. `object_store/collection.rs` считает SHA-1 и SHA-256, используемые retention. Прежние тесты корней и replay прошли. Задержки на больших деревьях и продолжительность удержания SQLite locks не измерялись. Объём одного прохода GC и повторные обходы legacy migration открыты для измерений.
- **F18 — PARTIALLY CONFIRMED, `7accf62`.** Типизированные причины сохранены на границах metadata HTTP, legacy launch и ZIP. `malformed_zip_preserves_the_archive_parser_cause` и тесты HTTP ошибок прошли. Остаточные предупреждения и совместимость опубликованных Rust-конструкторов требуют отдельного разбора перед удалением legacy helpers.
- **F21 — CONFIRMED, `d6d3fd9`.** `process_pin` фиксирует завершённый отказ, когда известно, что подготовка не создала процесса. Для неопределённого состояния сохраняется retention. Тесты `missing_game_executable_releases_all_pre_spawn_pins` и `a_later_marker_failure_releases_earlier_pins_and_retains_unknown_markers` прошли. Тесты неизвестного результата и дочернего процесса продолжают проходить. Нативная проверка Windows остаётся открытой.

## Итоговые проверки

| Среда | Проверки | Результат |
|---|---|---|
| Linux x86-64 | `fmt --check`, `check --locked --offline --all-targets`, `test --locked --offline`, `build --locked --offline` | PASS. 641 библиотечный тест, 2 CLI, 6 интеграционных, 18 doctests. Всего 667 passed, 0 failed, 1 ignored. |
| Linux x86-64 | `clippy --locked --offline --all-targets` | PASS с предупреждениями. Cargo отметил будущую несовместимость `proc-macro-error2`. |
| Python | `test_verify_platform.py` | 8 passed. |
| Windows x86-64 GNU | `fmt`, all-target check, `test --no-run`, build | PASS компиляции и линковки. Нативный runtime ожидает проверки. |
| Windows MSVC | Целевой Rust установлен. Итоговый прогон использовал GNU | Отдельный MSVC runtime и toolchain вне доказанного покрытия. |
| macOS | Native runner, Rust target и Apple SDK отсутствуют | Компиляция и runtime ожидают целевой среды. |

Логи `final-linux`, `final-windows-gnu`, `final-clippy.log` и `final-runner-tests.log`
сохранены в каталоге протоколов. В исходном sandbox-прогоне локальные HTTP
фикстуры упирались в ограничения среды. Последующие прогоны использовали
разрешённый loopback. Ошибка маршрутов в fixture F09 и промежуточный прогон с
нулём выбранных тестов исключены из доказательств. В итоговом прогоне flaky
сбоев не наблюдалось. Замеры повторяемости под нагрузкой не проводились.

## Сохранность и готовность к релизу

Тесты подтверждают сохранность файлов в проверенных сценариях отказа
переустановки, изменения CAS, неоднозначного legacy ownership и recovery.
Проверены SQLite restart, кросс-процессный отказ несовместимого bootstrap,
согласованность provenance и сохранение ручного выбора Java. Игровая JVM,
GUI, аварийное отключение питания и сетевые тома вне данного покрытия.

**Подключение к реальным пользовательским данным пока заблокировано.**
До релиза остаются полная гарантия границ файловых путей F02 и нативная
валидация исправлений, включая F07 и F14. На время проверки операции записи и GC на исторических
данных следует ограничить изолированными копиями. Обязательны нативные
Windows и macOS проверки, включая Windows managed Java, а также чистый
install → prepare → launch с отдельными тестовыми roots.

`.cargo/config.toml`, `Cargo.lock` и `not-a-database-url` сохранили исходные
контрольные суммы. Изменение `Cargo.toml` добавляет только Win32 Pipes feature.
Проверки выполнялись на временных SQLite и файловых деревьях. Коммиты локальные.

## Продолжение по F02, F07, F11 и F14

- **F02 — частичное исправление.** `files::directory` добавляет операции от удерживаемого каталога. Публикация и recovery контента используют корень, открытый при bootstrap, а CAS удаляет объекты и записи через удерживаемый каталог. `directory_publication_declines_a_changed_destination` прошёл на Linux. Остаются файловые вызовы вне этого контракта, включая части staging, распаковку и внешние установщики. Полная гарантия для них требует отдельного контракта владения и нативной проверки.
- **F07 — исправление, нативная валидация ожидается.** Миграция SQLite 0016 хранит намерение до файлового шага. Повторное открытие продолжает регистрацию только после подтверждения физической идентичности и содержимого собственной копии. `legacy_reopens_the_intent_and_recovers_only_the_published_file` проверяет повтор после сбоя SQLite и сохранение последующей пользовательской правки. При неопределённости намерение и файл остаются на месте.
- **F11 — подтверждённая часть исправлена.** Публичные каталоги провайдеров и каталог managed Java требуют HTTPS. HTTP сохраняется для явно переданного URL зеркала. Переход на HTTPS и перенаправление в пределах того же HTTP-origin допускаются. Сетевой клиент каталогов с ключами продолжает использовать прямые запросы. `redirects_follow_the_rules_for_https_and_configured_mirrors` и `url_policy_rejects_embedded_credentials_before_client_initialization` прошли.
- **F14 — исправление с консервативной стратегией, нативная валидация ожидается.** Эквивалентные исторические пути повторно используют существующую запись. Публикация и reconciliation откладывают неоднозначную привязку. Автоматического объединения исторических строк нет. `publication_reuses_the_binding_for_an_equivalent_historical_path` прошёл на Linux. Тест фактической эквивалентности имён для Windows и macOS ожидает нативного запуска.

Коммиты продолжения `64287b4` для F11 и `5a0b0ed` для файловой группы.
Намерение F07 создаётся для операций новой версии. Исторические операции,
завершившие файловый шаг до внедрения журнала, по-прежнему требуют
доказательства принадлежности копии.

Итоговый Linux-прогон продолжения прошёл с 672 тестами и одним ignored.
Форматирование, all-target check и сборка прошли. Clippy завершился с
предупреждениями. Windows GNU прошёл проверку, линковку тестов и сборку.
Нативные Windows и macOS остаются открытыми. В целевом прогоне прошли
24 проверки, а Python runner прошёл 8. Протоколы сохранены в
`/tmp/launcher-followup.ODRMTUVJ`. Контрольные суммы трёх ранее изменённых
пользовательских файлов совпали.

## F02. Повторная проверка файловых операций

Вердикт `CONFIRMED`. Статус `BLOCKED` для полного закрытия. Временные
каталоги и файлы CAS и контента создаются от удерживаемых каталогов.
Публикация CAS и игровых asset indexes использует тот же файловый
контракт. Forge получает проверенный каталог инстанса, а ядро
удерживает каталоги объявленных Maven-выходов на время работы процессора.

Распаковка ZIP модлоадеров сохраняет прежний файл до полной записи новой
копии. Распаковка ZIP и TAR managed Java создаёт выходные файлы от
удерживаемого каталога. TAR сохраняет режим обычных файлов и внутренние
ссылки поддерживаемого вида. Повторные имена файлов и ссылки с перенаправленным
родительским каталогом назначения отклоняются. Проверки выполнялись на
временных деревьях.

- `merging_archives_replaces_the_directory_entry_without_changing_linked_bytes`
  и `archive_extraction_declines_a_linked_output_directory` падали до
  изменений на проверках сохранности данных. Для второй проверки также
  подготовлена Windows-фикстура junction.
- `tar_extraction_preserves_relative_links_inside_a_runtime` проверяет
  относительную ссылку и идентичность hardlink после распаковки.
- `scratch_cleanup_preserves_a_replacement_directory` проверяет, что
  очистка оставляет каталог с неподтверждённым владением на месте.

Основные изменения и регрессии находятся в локальном коммите `2ed59d1`.
В контрольной группе прошли шесть целевых тестов, включая отмену импорта CAS.
Запуск из ограниченной песочницы показал отказ loopback-фикстур. Полный
Linux-прогон с доступом к loopback прошёл с 676 тестами и одним `ignored`.
Форматирование, all-target check и сборка прошли. Clippy завершился с
предупреждениями. Windows GNU проверен сборкой и линковкой тестов.
Python runner прошёл восемь тестов. Протоколы находятся в
`/tmp/launcher-f02.VFGbLLJA`. Исходные контрольные суммы `.cargo/config.toml`,
`Cargo.lock` и `not-a-database-url` совпали.

До полного закрытия остаются загрузка в файловый staging и отдельные
операции размещения библиотек и assets, публикация каталога managed Java,
рекурсивная очистка и граница полномочий внешнего Forge-процессора. Для
первых операций нужен сквозной контракт удержания каталогов до конца
записи или удаления. Для запуска Forge нужен поддерживаемый контракт
изоляции с сохранением совместимости установщиков. Нативные проверки
Windows и macOS обязательны перед изменением статуса.

## F02. Загрузки, размещение и публикация Java

Коммит `661d26b` закрепляет каталоги назначения на время загрузки
и публикации бинарных файлов. Основной путь установки передаёт открытые
каталоги Maven, assets и контента. Временные записи загрузчика сохраняют
прежний формат маркеров владения. Восстановление из CAS и запись URL-индекса
выполняются в удерживаемых каталогах. Скопированные библиотеки сохраняют
старый файл до успешного завершения записи. Перед чтением скачанных файлов
ядро проверяет, что это обычные файлы в выбранном каталоге.

- `copy_replace_refuses_a_redirected_destination_directory` воспроизвела
  изменение контрольного файла на исходной реализации и прошла после
  исправления. Для Windows подготовлена фикстура junction.
- `download_declines_a_cached_file_in_a_redirected_directory` проверяет путь
  повторного использования файла с совпадающим digest. `staged_download_declines_a_changed_destination`
  проверяет состояние назначения после отказа публикации.

Коммит `25c0feb` привязывает создание scratch и запись маркеров managed Java
к открытым каталогам.
Публикация поколения использует перенос в свободное имя. При отсутствии
поддержки такого переноса установка возвращает ошибку с сохранением
назначения. Windows использует перенос каталога в пределах одного тома.
[Контракт `MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw).

- `publication_preserves_a_replacement_generation_with_or_without_cancellation`
  проверяет, что изменившееся назначение остаётся на месте и не принимается
  за опубликованный runtime.
- `retention_preserves_a_replaced_partial_even_with_a_dead_owner` проверяет
  сохранность файла и маркера при изменении записи во время очистки.

Linux прошёл 681 тест, форматирование, all-target check и сборку. Один тест
помечен `ignored`. Clippy завершился с предупреждениями. Тесты выполнялись
на временных каталогах и SQLite. Windows GNU прошёл кросс-сборку и
линковку тестов. Python runner прошёл восемь тестов. Протоколы в
`/tmp/launcher-f02-placement.hKavwjmS`. Контрольные суммы
`.cargo/config.toml`, `Cargo.lock` и `not-a-database-url` совпали.

`BLOCKED` сохраняется для полного закрытия. Остаются рекурсивная очистка
поколений и scratch при конкурентных изменениях, отдельные старые адаптеры
и файлы метаданных с путями, а также граница полномочий процесса Forge.
Исторические маркеры частичных загрузок сохраняют прежний формат PID.
Очистка повторно проверяет идентичность открытой записи перед удалением.
Постоянной связи старого маркера с конкретным файлом этот формат не хранит.
Нативные Windows и macOS требуются для проверки файловых гарантий и
совместимости установщиков.

## F02. Очистка, карантин и метаданные

Основная группа изменений находится в `f3cf319`.

Запись метаданных использует временный файл и публикацию от удерживаемого
родителя. Кэши игровых манифестов и модлоадеров удерживают каталог и при
чтении, и при загрузке новой версии. Явно заданный корень метаданных может
оставаться ссылкой. Фикстура проверяет чтение действующего кэша через такой
override. Лог Forge публикуется отдельным файлом, поэтому старая hardlink
копия сохраняет байты.

Очистка scratch, каталогов natives, рабочих каталогов и карантинов
поколений выполняет удаления от открытого родителя. Перед переносом в
карантин и восстановлением проверяется ожидаемая запись каталога. Карантин
публикуется в свободное имя. Записи владения сохраняются до завершения
очистки дерева. Сборщик CAS сверяет дайджест с байтами открытого файла и
проверяет идентичность записи перед удалением.

- `metadata_write_refuses_a_redirected_parent` и
  `abandoned_scratch_cleanup_preserves_a_replacement` воспроизвели дефекты
  на исходной реализации на временных деревьях.
- `processor_logging_preserves_the_previous_file_when_linked` проверяет
  сохранность исходных байтов при штатном запуске тестового процессора.
- `unconfirmed_exit_retains_scratch_after_the_last_clone_is_dropped`
  проверяет сохранность natives при неизвестном результате процесса.

В `9350c7e` восстановление контента и копирование при миграции hardlinks используют
временные файлы, созданные от открытых каталогов. Маркеры процесса,
блокировки лаунчера и CAS, кэш списка версий, реестр Cleanroom и записи
операций managed Java проходят через тот же механизм. Сохранён формат
записей и маркеров. `marker_publication_declines_a_replacement_pin_directory`
проверяет отказ записи после смены каталога маркера.

Первый полный прогон дополнительной группы выявил смену префикса recovery.
Прежний префикс сохранён. Сборщик может убрать новые recovery-каталоги с
подтверждённым завершившимся владельцем. Исторические каталоги без маркера
сохраняются. `bootstrap_recovery_owns_lifetime_locks_and_finishes_sql_after_caller_drop`
прошёл в целевом и полном повторных прогонах без изменений теста.

На Linux прошли 687 тестов, один `ignored`. Форматирование, проверка всех
целей и сборка прошли. Clippy завершился с предупреждениями. Windows GNU прошёл
кросс-сборку и линковку тестов. Python runner прошёл восемь тестов. Протоколы
находятся в `/tmp/launcher-f02-ownership.eiaWRK6o`. Исходные контрольные суммы
`.cargo/config.toml`, `Cargo.lock` и `not-a-database-url` совпали.

**F02 остаётся `BLOCKED` по полному контракту аудита.** Повторная проверка
идентичности и сама файловая операция выполняются отдельными вызовами ОС.
Гарантия удержания родителя в проверенных путях опирается на владение
рабочим деревом и согласованную работу участников. Forge запускает
процессоры с правами лаунчера. Для изоляции нужен поддерживаемый механизм
ограничения внешнего процесса на всех целевых ОС и проверка штатных
установщиков. Нативные Windows и macOS пока недоступны.

## F02 и B01. Рабочее пространство процессоров и оставшиеся пути (21.11.2026)

Базовая точка: `689ef11`, чистое дерево. Сверка с handoff: путь репозитория,
SHA `.cargo/config.toml` и отсутствие `not-a-database-url` отличаются от
`docs/agent-handoff.md`. Пользователь утвердил текущее дерево как базу.
`Cargo.lock` = `45a050b1…2a3` до и после работы. Python runner удалён в
`382ce7d`, поэтому runner не выполнялся. Windows GNU по решению пользователя
не проверялся. Toolchain: rustc 1.99.1 через nix-shell, crates загружены
один раз `cargo fetch --locked`, дальше всё запускалось `--locked --offline`.
Базовый прогон совпал с handoff: 687 passed, 1 ignored.

**F02, коммит `6bc4b9a`.** Решение пользователя: сначала рабочее пространство,
ограничение средствами ОС — позже.

- Инвариант: процессор установщика не получает каталог игры инстанса и не
  наследует окружение лаунчера.
- Компонент: `modloader/forge/install.rs` и `processor.rs`,
  `instance/init/install.rs`.
- Исправление: `ROOT` указывает на приватный каталог внутри рабочего каталога
  установщика. Этот каталог создаётся от открытого дескриптора. Создание
  каталога инстанса перед finalize, которое было только у Forge, удалено:
  каталог по-прежнему создаётся при запуске. Окружение процесса очищается.
  Остаются только переменные локали на Unix (нужны для путей не в ASCII у
  старых JVM) и `SystemRoot`/`SystemDrive`/`windir` на Windows.
  `TMPDIR`/`TMP`/`TEMP` и `java.io.tmpdir` указывают на приватный `tmp`.
- Проверка: `processors_receive_a_private_root_inside_the_installer_work_directory`,
  `processor_environment_is_not_inherited_and_temporary_files_stay_private`.
  Linux: 691 passed, 0 failed, 1 ignored. fmt, check `--all-targets`, build и
  clippy прошли, clippy выдаёт предупреждения.
- Ограничения: процессор работает с правами лаунчера. Файловая система, сеть
  и потомки процесса средствами ОС не ограничены. Фактические пути записи
  в аргументах процессора не сверяются с объявленными outputs. Проверяются
  только объявленные outputs: по SHA-1 и по нахождению в Maven-каталоге
  поколения. Ранее загруженные библиотеки поколения повторно проверяются
  по SHA-1 при запуске. Native Windows/macOS не выполнялись.
  **Статус F02: `BLOCKED`** — нужен механизм ограничения процесса на каждой
  ОС и проверка штатных установщиков.

**B01, коммит `a8b3c97`.** Инвентаризация вызовов `files::child`: 66 мест в
production-коде. Два из них — явная запись по пути, 15 смешанных.

- Исправление: сброс кэша версий удаляет файлы от открытого каталога без
  следования ссылкам. Каталог операций managed Java создаётся от
  удерживаемого корня `.managed`.
- Проверка: `force_invalidation_preserves_files_behind_a_redirected_provider_directory`
  до изменения падала, после — прошла.
  `recovery_declines_a_redirected_operations_directory_without_touching_its_target`
  проходит и до изменения: более ранний шаг уже отказывает. Это контрактный
  тест, а не доказательство исправления.
- Остаётся: `ObjectStore` (`access_file`, `lock_exclusive`, `root_key`)
  создаёт корень по пути и намеренно следует через алиасы настроенного
  корня. Каталог игры и natives при запуске создаются по пути
  (`launch/resolved.rs`, `launch/natives.rs`), игра пишет в этот каталог
  сама. Другие места: `create_dir_all` в `assets/index.rs`,
  `assets/mod.rs`, `manifest.rs`, `archive.rs`, `forge/mod.rs` и
  `version/cache.rs`, а также ветка без lease в `ownership.rs`.
  Полное закрытие требует передать корневые дескрипторы из bootstrap во
  все эти места и решить судьбу алиасов настроенного корня.
  **Статус B01 без изменений**: обычный путь исправлен, конкурентное окно
  открыто.

Ручные и нативные проверки: `docs/manual-verification-plan.md`.

## B01. Открытые корни и удерживаемые каталоги (21.11.2026, продолжение)

Решение пользователя: алиасы настроенных корней сохраняются. Контракт:
настроенный корень (root override, `dirs.*`) может быть ссылкой и
разрешается один раз. Всё, что лаунчер создаёт под ним, открывается
покомпонентно от удерживаемого дескриптора без следования ссылкам.

- **`95c924b`** — `ObjectStore::beneath`/`child_of`: content store, asset
  CAS, managed Java (`.managed`, `.managed/bytes`, блокировки операций),
  runtime blobs. Корень хранилища открывается от якоря, очистка staging
  идёт от удерживаемого каталога. Регрессия
  `subpath_store_follows_a_configured_alias_but_not_a_redirected_owned_component`
  падает при прежнем разрешении по пути и проходит после исправления.
  Существующие тесты на алиасы проходят без изменений.
- **`ce88044`** — `Directory::open_beneath`/`open_child_of`. Через них
  создаются каталог инстанса и natives при запуске, при этом natives
  распаковываются в удерживаемый каталог. Тем же путём идут: ветка
  `ensure_instance_directory` без lease, кэш каталога версий и
  Maven-каталог поколения. Регрессии:
  - `prepare_declines_a_redirected_instance_directory_without_writing_through_it`
    падает с прежним созданием natives по пути;
  - `open_beneath_follows_the_configured_anchor_but_not_owned_components`.
- **`a3233fe`** — загрузка и распаковка установщиков Forge/NeoForge и
  Cleanroom, перенос встроенных библиотек и рабочий каталог процессора
  идут через удерживаемые каталоги `work` и `maven` поколения. Регрессия
  `installer_preparation_refuses_a_replaced_work_directory`: замена пути
  отклоняется, подложенный каталог не изменяется. Против прежнего кода
  тест не запускался, потому что изменился API контекста.

Linux: 695 passed, 0 failed, 1 ignored. fmt, check `--all-targets`, build и
clippy прошли, число предупреждений равно базовому. `Cargo.lock` не изменился.

**Статус B01 без изменений.** Остаются вызовы `Directory::ensure_path` с
путём. Такой вызов следует через существующие промежуточные компоненты и
отказывает только на последнем:

- `utils/download/destination.rs` (`Destination::Path`);
- `utils/process_pin.rs`, `utils/process.rs`, `utils/files/mod.rs`
  (запись файла по пути);
- `instance/download/artifacts/mod.rs`, `assets/storage.rs`;
- `instance/init/generation_gc.rs`, `java/managed/extract.rs`;
- `modloader/cleanroom/releases.rs`, `modloader/repository.rs`,
  `modloader/forge/output.rs`;
- `utils/files/placement.rs`, `instance/content/staging.rs`.

Для каждого места нужно передать якорь или удерживаемый каталог. На Windows
(`not(unix)`) операции `Directory` по-прежнему выполняются по пути. Нативная
проверка Windows и macOS не выполнялась.

### B01. Закрепления процессов и оставшиеся вызовы (`f325c67`)

- `ProcessPin::create_in` создаёт закрепление в удерживаемом каталоге
  ресурса. Если каталог больше не находится по своему пути, создание
  отклоняется. Так работают: managed Java (каталог хранилища), runtime при
  запуске (каталог runtime store) и процессоры Forge (поколение установки
  передаётся через `FinalizeRequest`). Регрессия
  `pins_refuse_a_resource_whose_ancestor_was_replaced` падает с прежним
  закреплением по пути и проходит после исправления.
- Через `open_child_of` теперь открываются кэш метаданных модлоадеров,
  реестр Cleanroom и запасная ветка asset storage. Различающего теста нет:
  прежний код уже отказывал на ссылке в последнем компоненте, а единственный
  промежуточный компонент — настроенный якорь. Импорт архива Cleanroom
  загружает файл в удерживаемый временный каталог.
- Linux: 696 passed, 0 failed, 1 ignored. Clippy выдаёт одно новое
  предупреждение `too_many_arguments` для `forge::install::run_retained`;
  оно записано, не подавлено.

Итог проверки после этой группы. Во внутренних production-потоках ядра
запись под настроенными корнями идёт через удерживаемые каталоги, на Unix
покомпонентно без следования ссылкам. Оставшиеся вызовы
`Directory::ensure_path` делятся на две группы:

- только тестовые (`#[cfg(test)]`);
- публичные функции, принимающие путь от вызывающего, без внутренних
  production-вызовов: `utils::files::child` (возвращает путь),
  `utils::download::transfer::{download, place}`, `utils::files::atomic_write`,
  `instance::download::artifacts::hydrate`, `Natives::extract`.

**Статус B01 без изменений.** Для закрытия нужны:

1. решение о видимости этих публичных функций: сделать `pub(crate)` или
   задокументировать, что путь задаёт вызывающий. Решение затрагивает
   потребителя GUI;
2. нативная проверка Windows. На `not(unix)` операции `Directory` выполняются
   по пути, родительская цепочка удерживается открытой без delete sharing;
3. повторный эксперимент с конкурентной подменой из `bc1945d`, на этот раз
   против сервисного API, а не против `files::child`.
