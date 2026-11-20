# B02 — Проверяемая migration исторических hardlinks

После F04 существующий записанный путь тоже требует подтверждённой физической
identity с CAS. Отсутствие CAS, ошибка получения identity и неоднозначность
оставляют файл и SQL-привязку на месте. Ниже описан исторический checkpoint.
Текущие ограничения восстановления и нативных проверок указаны в
[журнале remediation](launcher-core-remediation.md).

Checkpoint: **19 ноября 2026**, исходный core HEAD `7d9ac3a` плюс code/tests этого
шага. Этот commit (`fix(content): resolve Windows physical file identity for legacy
hardlink migration`) реализует Windows physical identity: `same_physical_file` получает
path-based contract с Unix device+inode, Windows volume serial + 128-bit file ID и
fail-closed `None`. **B02 остаётся частичным**: доказанное разъединение расширено,
неизвестное владение не угадывается. Linux pipeline green (620+2+2+18); native Windows
execution pending — его выполнит пользователь на dual-boot. GUI не изменялся, не
запускался и не проверялся.

## Исправленные пробелы

1. Migration `171b9db` разъединяла только существующий записанный путь. Внешний
   rename/disable оставлял hardlink в экземпляре, через который можно менять CAS.
   Последующий поиск индексатора по digest не является proof physical ownership.
2. Migration guards принадлежали bootstrap caller, а blocking copy могла пережить
   его drop. Теперь migration — owned `Tasks` worker; три lifetime locks переданы
   через `Arc`, shared CAS admission и activity guard удерживаются до SQL completion.
   Shutdown ждёт допущенную migration, не abort-ит copy/SQL из-за исчезновения caller.

Production изменения ограничены `src/instance/content/store/legacy.rs` и startup
wiring `src/instance/content/service.rs`. Public API, schema/migrations, providers,
dependencies, numbered GUI patches и local lockfile не меняются.

## Контракт выбора и разъединения

- Вход — существующая SQL hardlink-привязка к object и зарегистрированному instance.
  Обычный **записанный regular path имеет приоритет**, как раньше: текущие bytes
  копируются независимо от наличия CAS. Symlink/unsafe path не принимается.
- Только если записанный путь отсутствует, проверяется текущий regular CAS file
  по layout SHA-256. Digest — адрес поиска, **не proof bytes/ownership**. CAS не
  восстанавливается и не перезаписывается этой migration.
- Rename proof реализован на **Unix: совпадение device + inode** и на **Windows:
  volume serial + 128-bit file ID** (`GetFileInformationByHandleEx`/`FILE_ID_INFO`,
  fail-closed `None` при сбое query). Размер, timestamps и равные hashes не заменяют
  identity. На остальных платформах rename lookup возвращает false; обычные записанные
  paths по-прежнему разъединяются.
- Windows identity-файл открывается через `std::fs::OpenOptions` + stable
  `OpenOptionsExt` (`access_mode(FILE_READ_ATTRIBUTES)`, `share_mode(READ|WRITE|DELETE)`,
  `custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)`) — чтобы match'овать `symlink_metadata`
  no-follow. Windows-поля `std::MetadataExt` unstable → windows-sys единственный stable
  route (feature `Win32_Storage_FileSystem`). `remove_file`-перед-`persist` из scope
  исключён как доказанно избыточный: `tempfile::persist` pass'ом `overwrite=true`
  (`MoveFileExW REPLACE_EXISTING`) и std `rename` REPLACE существующий destination
  (Win10 1607+ = Unix semantics), production `detach()` unchanged.
- Поиск ограничен известным `inventory_scope` того же ContentKind: непосредственные
  JAR/`JAR.disabled` в mods либо ZIP в resourcepacks/shaderpacks. Symbolic links,
  symlinked inventory directory, nested/unknown archives и files вне scope не дают
  ownership. Путь, не представимый SQL UTF-8 строкой, не преобразуется lossily.
- Нужен **один физический alias и один возможный historical binding** в данном
  instance/object/kind. Несколько aliases или конкурирующие hardlink bindings
  означают defer, а не выбор первой строки SQL. Занятый destination path_key не
  объединяет bindings, даже если bytes совпадают.
- Planned FileStamp проверяется непосредственно перед copy, против открытого file
  и после copy. Обнаруженное изменение/replacement откладывает migration. Это не
  sandbox от внешних concurrent writers/подмены предков между syscalls.
- Текущие bytes копируются в соседний tempfile, permissions сохраняются, файл
  синхронизируется и persist заменяет именно доказанный legacy path. Его исходные
  bytes не меняются; последующие edits экземпляра не меняют CAS через этот alias.
- Если SHA-256 скопированных bytes совпал с зарегистрированным object, placement
  становится `copy`, object/provider identity сохраняются. Если исходный hardlink
  уже испортил CAS, текущие bytes сохраняются как `external`, object/preferred
  provider очищаются. Corrupt CAS остаётся corrupt: migration его не «ремонтирует».
- SQL relocation сохраняет binding ID/instance/kind/pinned/seen_generation, меняет
  relative_path + exact path_key, учитывает enabled/disabled suffix и present status,
  сбрасывает stale observations/indexed fingerprint. Provider metadata не меняется.
  UPDATE повторно проверяет прежние ID/object/path/key/kind/placement, свободный key
  и отсутствие нового competing claim; изменившаяся запись не присваивается заново.

FS persist и SQL UPDATE **не единая транзакция**. SQL failure/collision после copy
оставляет независимые bytes и исходную SQL-привязку; CAS и чужие bindings не
перезаписываются. Worker логирует ошибку даже после drop caller; constructor
сохраняет прежний warning/defer policy. Это не отдельный durable migration journal:
после failure/process exit в этом окне renamed copy может уже не иметь physical
identity CAS, и повторный startup не обязан автоматически перенести её binding.
Позднейшая сверка/прикладное исправление — отдельный шаг, не guessing migration.

## Regression evidence

`src/instance/content/legacy_tests.rs` подключён из `api_tests.rs` и использует
temp roots, настоящие files/hardlinks и SQLite. **18 tests реально прошли на Linux**
(в этом commit их identity-assert'ы заменены на `legacy::same_physical_file`-path вызовы;
семь gates `cfg(unix)` → `any(unix, windows)`, `dropped_bootstrap` ungated с portable
lock API). Два unix-only byte-name/symlink fixtures остались unix-only. Native Windows
execution pending.

| Проверка | Regression(s) |
|---|---|
| Rename/disable, сохранение ID/pin/metadata/permissions, независимые edits | `unique_renamed_and_disabled_links_keep_binding_identity_and_provenance` |
| Resource/shader scopes, сохранение bytes/inode вне scope | `renamed_archives_are_found_only_in_their_registered_content_scope` |
| Равные bytes/size/mtime не ownership | `equal_bytes_size_and_mtime_do_not_prove_renamed_hardlink_ownership` |
| Несколько aliases / несколько historical claims | `ambiguous_inode_aliases_are_not_detached_or_rebound`, `multiple_historical_bindings_cannot_claim_one_remaining_alias` |
| Collision до/после copy и новое competing claim/изменившиеся key/kind | `an_occupied_renamed_path_keeps_both_bindings_and_the_alias_unchanged`, `a_collision_after_copy_cannot_merge_or_replace_bindings`, `changed_binding_or_new_competing_claim_after_copy_is_not_reassigned` |
| Missing/replaced CAS, equal bytes не заменяют physical proof | `missing_cas_does_not_turn_equal_digest_lookup_into_ownership`, `a_replaced_cas_file_with_equal_bytes_does_not_identify_the_old_alias` |
| Записанный path при missing CAS | `a_recorded_path_can_be_isolated_even_when_cas_is_missing` |
| Modified/corrupt CAS alias, очищенная stale identity, сохранённые bytes | `a_corrupt_renamed_cas_alias_preserves_current_bytes_and_drops_stale_identity` |
| Unknown/nested/outside/non-Unicode aliases и links | `aliases_outside_the_known_archive_inventory_are_left_untouched`, `a_non_unicode_alias_is_not_lossily_rebound`, `symbolic_links_and_symlinked_inventory_directories_are_not_ownership` |
| Target replacement между lookup и copy | `a_replaced_target_after_identity_lookup_is_not_overwritten` |
| Caller drop, real SQLite write lock, shutdown wait и release всех guards | `dropped_bootstrap_keeps_migration_guards_until_sql_completion_and_shutdown` |
| SQL failure после copy не теряет bytes/не меняет CAS/binding | `sql_failure_after_isolation_preserves_bytes_cas_and_original_binding` |

Отдельно объявлен non-Unix regression
`unsupported_physical_identity_defers_even_a_unique_renamed_link`: теперь `not(any(unix, windows))`
— скомпилирован только для platform'ов без physical identity (Windows больше не входит).
Семь identity tests и caller-drop `dropped_bootstrap` (recorded path + portable SQL/lock
API; `File::lock` stable с 1.89) — `any(unix, windows)`: реально выполняются на Linux и
скомпилированы/link'ы для Windows GNU; native Windows execution pending (дual-boot).

Checkpoints `legacy-identified` и `legacy-sql` operation-scoped, только `cfg(test)`.
В caller-drop test bootstrap действительно доводится до copy/SQL boundary; затем
удерживается `BEGIN IMMEDIATE`, caller удаляется, проверяются activity launch/install/
content, оба launcher/instances locks, CAS ownership и shared admission. Shutdown
pending при locked SQL; после rollback он ждёт завершение migration, проверяются
SQL copy placement, освобождение всех locks и независимое редактирование bytes.
Никакая game/JVM/GUI fixture не запускается.

Первый check выявил отсутствие SQLx traits у публичного ContentKind: SQL string
декодируется только в migration adapter. Следующие compile failures в новых edits
(borrowed/moved key/path и неверное имя поля test FileStamp) исправлены до PASS.
Первый выполненный focused run: **13 PASS / 1 FAIL** — test ошибочно ожидал прежний
ctime у remaining alias. Разъединение соседнего hardlink закономерно меняет shared
inode ctime/link count; test теперь проверяет прежние device/inode/mtime и bytes.
Повтор expanded focused suite: **18 PASS**, затем full pipeline PASS.

В этом commit path-based `same_physical_file` заменил device/inode/byte-name asserts,
`outside_stamp` binding восстановлен перед setup-pair assert'ом, и убран дубль
`fs::read(&target)` в `dropped_bootstrap`; пере-сфокусированный run: **18 PASS**,
gate flips не изменили Linux execution count.

## Проверки и артефакты

| Pipeline | Фактический результат |
|---|---|
| Linux x86_64 locked/offline fmt/check --all-targets/test/build | **PASS: 620 library + 2 CLI + 2 integration + 18 doctests; 0 failed, 1 ignored** |
| Windows GNU cross из Linux, fmt/check --all-targets/test --no-run/build | **Compile/link PASS**, execution не выполнялся (cross runner compile-only) |
| Windows native — dual-boot пользователь | Pending: выполнит пользователь `verify-platform.py --output <new dir>` на Windows; commit land'ится с native pending |
| Windows MSVC/macOS native | Не запускались в B02; прежние toolchain/native runner blockers сохраняются |
| GUI | Не запускался/не проверялся по уточнению пользователя |

Full tests включают прежние startup migration, modified-link/provenance, recovery,
index reconciliation, admission и process-crash regressions. Их прохождение не
добавляет power-loss гарантию или новую subprocess crash coverage для B02 migration.

```sh
bash docs/check.sh test legacy_tests
# этот commit — offline native + Windows GNU cross (новые dirs):
python3 docs/verify-platform.py --offline --output /tmp/opencode/core-b02-native-2
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-gnu \
  --output /tmp/opencode/core-b02-windows-gnu-2
# native Windows evidence — выполнить на dual-boot Windows (не cross):
python docs/verify-platform.py --output /tmp/opencode/core-b02-native-windows
```

Output directories уже существуют, для повтора выбрать новые. Raw evidence:

- `/tmp/opencode/core-b02-native/{fmt,check,test,build}.log`, `report.json` (прежний run).
- `/tmp/opencode/core-b02-windows-gnu/{fmt,check,test-compile,build}.log`, `report.json`
  (прежний run).
- `/tmp/opencode/core-b02-native-2/{fmt,check,test,build}.log`, `report.json` —
  **этот commit**, offline native, `lock_unchanged: true`.
- `/tmp/opencode/core-b02-windows-gnu-2/{fmt,check,test-compile,build}.log`,
  `report.json` — **этот commit**, Windows GNU cross, `lock_unchanged: true`.
- `/tmp/opencode/core-b02-native-windows/` — pending: native Windows evidence от
  dual-boot пользователя.
- `/tmp/opencode/core-b02-regressions-{first,second,verified}.log`, initial check и
  existing-migration compile logs; промежуточные failures не скрыты.
- `/tmp/opencode/core-b02-baseline.json`, `core-b02-preservation.json`: GUI 173
  source files/status, numbered patch hashes, core local lock, prior untracked file.

Logs/reports локальные, не durable committed archive. Их UTC timestamp сохранён
по часам окружения, не переписан под локальную дату checkpoint. Local Cargo.lock
SHA-256: `45a050b11dd430a83528d17c98f474c26f767a039eebec35dac61df3951cf2a3`.
Windows feature-add (`Win32_Storage_FileSystem` к существующему `cfg(windows)`
windows-sys) — single feature, byte-unchanged lock; оба runner'а этого commit'а
подтвердили `lock_unchanged: true`. Lock и `not-a-database-url` не входят в B02 commit;
исходный GUI и patches 01–05 сохранены. Push/CI dispatch не выполнялись.

## Что остаётся открытым

Unknown/unregistered aliases, multiple candidates/claims, unsupported physical
identity, moved/nested files вне scope, SQL failures в FS/SQL окне и произвольная
внешняя подмена FS/DB не решаются угадыванием. Наличие других aliases вне scope
может по-прежнему позволять менять CAS, хотя доказанный path уже изолирован.
Corrupt CAS repair/ownership migration и более агрессивный GC этим шагом не
добавлены. Windows physical identity реализован (path-based contract); native Windows
execution pending — выполнит пользователь на dual-boot.
**B02/B06/D08/S05/S06 и native/manual P11 остаются частичными**.
См. [publication contract](publication-contract.md), [retention contract](retention-contract.md)
и [platform evidence](platform-verification.md).
