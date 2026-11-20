# D08 follow-up — strict asset index retention roots

Этот task commit (`fix(gc): reject ambiguous asset index retention metadata`)
устраняет конкретный fail-closed дефект private asset CAS collector, **не**
расширяет ownership или право удалять Mojang/virtual/resource placements.
D08/S05/S06 и native/manual P11 остаются частичными.

После исправлений F01/F09 установка проверяет SHA-1, размер и схему сырого
индекса. Проверенный URL-кэш хранится под `meta/asset-indexes`, а Minecraft
получает индекс из `assets/indexes/<id>.json`. Публикация допускает повтор
тех же байтов и отклоняет конфликт ID с сохранением старого индекса. SQL-профиль
фиксируется после игровой публикации. Подготовка к запуску повторно проверяет
индекс для профилей с признаком верификации. Поддержка исторических virtual
и resources projections остаётся отдельным контрактом.

## Дефект и проверяемый контракт

Прежний `serde_json::Value` parser терял первые roots при дублирующихся ключах:
два `objects`, два одинаковых object names или два `hash` оставляли только
последнее значение. Collector мог объявить первый intended hash unrooted и
удалить immutable CAS bytes. Кроме того, известные `virtual`, `map_to_resources`
и `size` поля допускали любой JSON type: неизвестная схема считалась понятной.
Это противоречило заявленному corrupt/unknown-metadata gate.

`src/instance/download/assets/index.rs` задаёт общую схему установки и retention:

- Required `objects` — JSON map с уникальными именами, **не** last-wins map.
- Каждый object имеет required string `hash`; SHA-1 validation и lowercase roots
  остаются в `add_hash`. Duplicate object fields отвергаются.
- Optional flags — boolean; optional `size` — `u64`. Отсутствие поля допускается
  для historical indexes, но явно присутствующий `null` не считается отсутствием.
  Отрицательные/fractional/overflow values и чужие types отвергаются.
- Duplicate top-level fields, object names и known object fields, unknown/future
  fields, invalid JSON и non-object layouts дают error, не partial roots.
- Existing bounded regular-file read (32 MiB), no-links/layout checks, complete
  owner catalogue, all-owner SQLite locks и private CAS maintenance сохранены.
  Любой schema failure происходит **до** `Collection::plan`/новых intents/unlink.
  При replay existing intents не дают permission обойти текущий invalid snapshot.

`size` — metadata, **не** identity/ownership/byte proof; даже valid size mismatch
не снимает intended checksum root. `virtual=true`/`map_to_resources=true` не
разрешают удаление или признание virtual/resource layouts owned. Их фактические
unknown directories по-прежнему блокируют collector. Установка и retention
декодируют исходные байты одной схемой.

## Регрессии

Пять новых integration tests в
`src/instance/download/assets/retention/tests.rs`, реальные temporary files/SQLite:

1. Два известных flags × пять unknown JSON types: collector error и отсутствие
   новых intents; unrelated CAS bytes сохранены.
2. Семь invalid size types: rooted и unrelated bytes сохранены до intents.
3. Шесть duplicate variants: top-level objects, object-name, hash, size, virtual,
   map_to_resources. Ни потерянный первый root, ни unrelated bytes не удаляются.
4. Пять valid controls: omitted flags/size, false/true, zero/u64::MAX size. Index
   root остаётся; только unrelated private CAS garbage удаляется, index bytes
   неизменны. Maximum size тестирует representation, не обещает artifact validity.
5. Три replay cases (duplicate/flag/size) после **реального process exit** на
   durable intent boundary. Invalid snapshot оставляет bytes и intent неизменными;
   fresh valid rooted snapshot снимает stale intent без unlink; только последующий
   valid unrooted snapshot позволяет collection. Subprocess — controlled fixture,
   не JVM/game и не power-loss test.

Первый pre-fix запуск `core-d08-index-red.log` (task-код уже в рабочем дереве,
прежний `serde_json::Value` parser временно возвращён): **1 PASS / 4 FAIL**. Прошёл
только valid control; malformed flags, malformed size, duplicate roots и replay-случай
нарушили fail-closed. Red/green закрыт фактическими прогонами, первый и повторный
дают одинаковый результат: focused `core-d08-index-focused.log` →
`core-d08-index-focused-final.log` — 5 passed; `core-d08-index-retention.log` →
`core-d08-index-retention-final.log` — 17 passed; полный suite
`core-d08-index-full-suite.log` → `core-d08-index-full-suite-final.log` —
**620 library + 2 CLI + 2 integration + 18 doctests passed, 0 failed, 1 ignored**.

## Воспроизведение и evidence

```sh
bash docs/check.sh test --lib asset_index_
bash docs/check.sh test --lib instance::download::assets::retention
python3 docs/verify-platform.py --offline --output /tmp/opencode/core-d08-index-native
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-gnu \
  --output /tmp/opencode/core-d08-index-windows-gnu
```

Output directories новые; runner не перезаписывает предыдущие reports. Raw logs
в `/tmp/opencode`: `core-d08-index-{red,focused,retention,focused-final,retention-final,full-suite,full-suite-final}.log`
и `core-d08-index-{native,windows-gnu}/` — native PASS, Windows GNU compile/link без
execution. Они локальные, не committed archive.
Reports относятся к base HEAD `9e17fff` + working D08 edits, не к clean base.

Verified code mapping `/tmp/opencode/core-d08-index-code-hashes.json` (SHA-256
`200710628a54b1e8eaf96f59354311da4532178fa376afe3ea83df1d7b2bb103`): sha256 трёх
task-файлов и manifest-hash строки инвентаря, без timestamps. После commit оно
regenerated от committed bytes и сравнивается с этим значением.

Preservation сверяется по паре D08-инвентарей: `core-d08-index-core-before.json`
(18:57 UTC) и `core-d08-index-core-after.json` (19:37 UTC; прежде файл с именем
`--repo-root` в корне репозитория — argv инвентарного скрипта, вынесен в
`/tmp/opencode`). Совпадают 173 GUI files, GUI status, source-only patches 01–05,
локальный core `Cargo.lock` и `not-a-database-url`; различаются только recorded_at и
core porcelain, где в позднем добавлена строка самого файла инвентаря. Ни
`Cargo.lock`, ни `not-a-database-url`, ни инвентарные файлы не входят в task commit.
Исходные B06 inventories (`core-b06-baseline.json`, `core-b06-gui-baseline.json`)
как inputs больше не на диске — basis проверки это пара инвентарей выше. GUI не
запускается.

## Что не закрыто

Historical/unknown ownership, missing/replaced catalogue DB, corrupt CAS metadata,
unknown layouts и future versions по-прежнему defer collection. Более агрессивный
SHA-1/virtual/resource cleanup требует доказуемой ownership migration; этот commit
её не подменяет. Real JVM/descendant retention, external writers/FS/DB replacement,
native Windows/macOS execution, manual GUI verification и power-loss не PASS.
