# B06 — installed-aware dependency resolution

## Реализованный контракт

Этот task commit (`fix(content): resolve dependencies of fixed installed releases`)
продолжает `44c7dfe`. B06 **остаётся частичным** для неизвестной provider identity и
исторических записей без dependency metadata; их происхождение не угадывается.
GUI, live providers и JVM этим шагом не запускаются.

1. `InstanceContentService::plan_install` строит installed seeds только из
   согласованных SQL object/provider bindings и полного inventory SHA-256 snapshot.
   Активным считается наблюдаемый `.jar`, а не исторический SQL enabled flag.
   `.jar.disabled`, missing/modified/unstable файлы и неоднозначный rename не дают
   resolver metadata. Existing unique-hash rename policy сохранена: это byte identity
   для resolution, **не** доказательство физического ownership для migration/GC.
2. Каждая активная известная запись имеет явный dependency список:
   SQL `NULL` → typed `ContentError::MissingDependencyMetadata(ReleaseKey)`;
   JSON `[]` → известное отсутствие связей;
   повреждённый JSON, JSON `null`/object и malformed dependency → `ContentError::Json`.
   Missing metadata блокирует планирование также для unrelated roots: нельзя
   обещать совместимость с непроверенными связями активного installed seed.
   Планирование не пишет metadata в SQL и не подставляет данные похожего/нового релиза.
3. Fixed installed releases остаются selected на всём пути backtracking. Общая
   `installed_requirements` проверяет map/candidate project identity, отвергает
   `Unknown` relation и добавляет собственные `Required` edges **всех** seeds к
   требованиям solver и к bounded catalog traversal — не только seeds из roots.
   Транзитивные связи проходят обычный candidate search. `Incompatible` проверяется
   в обе стороны; exact release ограничение не заменяет другой installed release.
4. Требования deduplicate по `(provider/project, exact_release)`: разные exact
   ограничения не сливаются. Cycles не выбирают fixed release заново. Seeds
   обходятся в стабильном provider/project порядке, независимо от HashMap iteration.
   Optional/Embedded/Tool/Include не превращаются в обязательные downloads.
5. Installed projects не запрашиваются у provider заново и не попадают в download
   результат. Остальные зависимости используют тот же общий `RequestBudget`
   (128 actual adapter HTTP requests), общий exact cache и 4096 solver decisions.
   Existing root validation и 90-second planning timeout сохранены.
   404/unavailable exact candidate не становится общим transport failure;
   HTTP 503/transport/limit failure не называется dependency conflict.
6. Несколько активных bindings одного project не молча перезаписывают друг друга:
   разные release IDs или разные dependency списки дают `ContentError::Invalid`.
   Совпадающие release/metadata deduplicate. Это консервативное сравнение списков,
   не provider metadata reconciliation и не доказательство взаимозаменяемости JAR.

## Регрессии

**25 новых тестов:**

| Слой | Количество | Контракты |
|---|---:|---|
| `src/instance/content/tests.rs` | 7 | Required от unrelated seed; exact/transitive/cycle; Unknown; fixed exact conflict; dedup и decision limit; non-required kinds; invalid seed identity; provider namespaces |
| `src/instance/content/resolve/catalog_plan_tests.rs` | 8 | Exact/transitive traversal без seed requests; Unknown до lookup; fixed conflict; missing required conflict; HTTP 503; shared 128 budget; late-exact backtracking/cache; exact/project identity validation |
| `src/instance/content/installed_tests.rs` | 10 | Real SQLite/files: NULL vs [] vs corrupt JSON; Unknown; fixed conflict; modified/disabled/missing/ambiguous metadata exclusion; unique rename; conflicting release/metadata и identical duplicate bindings |

Catalog tests исполняют **тот же** private generic traversal, что production
`ContentCatalog`, через private `ReleaseCatalog` seam. Fake catalog не вызывает
HTTP, записывает точные requests/compatibility и использует production
`RequestBudget::charge_request`; один fake lookup моделирует один budget charge,
не все normalize subrequests реальных adapters. Production adapters по-прежнему
сами начисляют каждый actual HTTP request через общий budget. Existing adapter
normalization/cache tests остаются отдельным evidence, не live-provider проверкой.
Seam не экспортирован и не расширяет external catalog API.

Первый запуск до исправления (`core-b06-red.log`) дал **7 PASS / 3 FAIL**:
seed Required graph возвращал только 2 вместо 4 selections; seed Unknown обходил
rejection; `NULL` dependencies трактовались как пустые. Explicit `[]` и corrupt
JSON controls прошли. Промежуточные compile errors inference/test helper исправлены;
последующий focused/full результат записан ниже, предыдущие logs не перезаписаны.

## Проверка и evidence

Воспроизведение (output directories должны быть новыми):

```sh
bash docs/check.sh test --lib installed_
python3 docs/verify-platform.py --offline --output /tmp/opencode/core-b06-native
python3 docs/verify-platform.py --offline --target x86_64-pc-windows-gnu \
  --output /tmp/opencode/core-b06-windows-gnu
```

Linux native core: **fmt / check --all-targets / test / build PASS**;
**615 library + 2 CLI + 2 integration + 18 doctests passed**, 0 failed, 1 ignored
(`java::utils::tests::probes_java_on_path`, не PASS). Focused `installed_`: **30 PASS**,
включая все 25 новых tests и 5 прежних совпадений filter.
Windows GNU (`x86_64-pc-windows-gnu`): **fmt / check --all-targets /
test --no-run / build compile/link PASS**, без execution.
Reports указывают base HEAD `77e5d34` и B06 working edits. Verified code fingerprint
в `/tmp/opencode/core-b06-code-hashes.json` связывается с task commit после commit.
SHA-256 sorted code-hash mapping:
`a322a6803cd24f219ef0fda23653bf6b8e3ae0dc117ea0d8436fe1058405fce4`.

Локальные raw evidence (не committed durable archive):

- `/tmp/opencode/core-b06-{red,focused,focused-repeat,focused-final}.log`;
- `/tmp/opencode/core-b06-native/{fmt,check,test,build}.log`, `report.json`;
- `/tmp/opencode/core-b06-windows-gnu/{fmt,check,test-compile,build}.log`, `report.json`;
- `/tmp/opencode/core-b06-{baseline,gui-baseline,code-hashes,preservation}.json`.

Исходные 173 GUI files/status, patches 01–05, local core lock и прежний untracked
`not-a-database-url` проверяются до/после. Local lock SHA-256:
`45a050b11dd430a83528d17c98f474c26f767a039eebec35dac61df3951cf2a3`.
Lock, dependency switch, GUI patches и user baseline в B06 commit не входят.

## Открытые границы

- Historical dependencies автоматически не восстанавливаются. Typed failure —
  доказуемый fail-closed контракт, **не** completed metadata migration/repair flow.
- Unregistered/unknown/cross-provider ambiguous identity не становится known seed
  только по имени файла, archive hints или provider lookup. Preview не является
  гарантией совместимости с модами, identity которых неизвестна; отдельного UI
  acknowledgement/warning flow этим шагом не добавлено. Поэтому B06 не blanket PASS.
- Existing snapshot staleness guard сохранён; arbitrary concurrent FS/DB replacement
  и внешние writers, не соблюдающие admission, не объявлены безопасными.
- Native Windows/macOS execution, GUI runtime/manual verification, live providers,
  реальные JVM/descendants и power-loss остаются без evidence. Cross `--no-run`
  не является выполнением Windows tests.
