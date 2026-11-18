# Контракт публикации и восстановления

## Гарантия и её границы

Текущий контракт ядра — **восстановление после завершения процесса**, а не
устойчивость к отключению питания, падению ОС или потере диска. `Completed`
означает, что файловые операции и SQL commit вернули успех. Это не единая
транзакция SQLite + filesystem и не гарантия физического сохранения directory
entries. `atomic_write` и `publish_new` обещают атомарную видимость отдельного
файла/отсутствие перезаписи, но не power-loss durability.

Предполагаются доступные прежние SQLite и launcher root, локальная файловая
система с обычной семантикой rename/link и отсутствие внешней конкурентной
подмены каталогов. Изменённый пользователем конечный файл сохраняется;
автоматическое восстановление останавливается с конфликтом. Потерянный или
повреждённый CAS диагностируется, а не заменяется непроверенными байтами.

## Add-only установка контента

1. При удержании activity lease и блокировок launcher/store записывается
   `preparing`. До этого не проверяется/не импортируется CAS контента.
2. Lookup объекта и его lease выполняются одной SQL-транзакцией **до** чтения
   кешированных байтов. Registration/adoption и lease также атомарны. Пока hash
   нового импорта неизвестен, `preparing` защищает весь store: будущему GC нельзя
   игнорировать окно filesystem import → SQL registration.
3. Копии и downloads готовятся в приватном `.sonata-content-*` вне `mods`;
   полный PID marker атомарно публикуется до полезных файлов. Имена провайдера
   находятся в отдельном `files`, не рядом с marker. Проверяются хеши и snapshot.
   Отмена/ошибка до intent удаляет только `preparing` и его leases. Panic/exit
   может оставить их до следующего старта. Startup cleanup разрешён только для
   соответствующего canonical store root под блокировкой самого store.
4. Одна SQL-транзакция переводит `preparing → publishing`, сохраняет версионированный
   список файлов и leases. После этого отмена не прерывает публикацию.
5. Новые конечные файлы публикуются no-clobber. Между файлами процесс может
   завершиться: journal остаётся источником forward recovery.
6. После всех файлов одна SQL-транзакция создаёт bindings/provenance, переводит
   `publishing → completed` и освобождает leases. История журнала затем удаляется.
   Startup recovery повторяет только `publishing`; уже совпавшие файлы принимаются
   как external, отсутствующие создаются проверенными копиями CAS.

После переноса root ключ старой подготовки намеренно не используется для очистки
другого root: такая запись остаётся консервативным блокером. Ключ — идентичность
ресурса блокировки, не путь к байтам и не семантическая идентичность объекта.

## Другие подсистемы

- Immutable CAS синхронизирует файл перед persist; aliases — проверяемые hints.
  Директории после persist/rename не синхронизируются. Каждый новый импорт имеет
  приватный `staging/import-*` с атомарно опубликованным PID marker. Cleanup
  удаляет только staging подтверждённо завершившегося owner, никогда published
  objects/aliases/старые плоские staging-файлы.
- Runtime install публикует профиль только после проверки поколения; старый
  профиль сохраняется при обычной ошибке до SQL publish. Это также не power-loss
  транзакция. Retention старых поколений требует отдельного ownership-контракта.
- Managed Java имеет собственный persistent journal и recovery; данный документ
  не расширяет его гарантии до power-loss durability.
- Оба downloader атомарно публикуют полный `<partial>.sonata-owner` через
  no-clobber до самого partial. Недописанный числовой префикс PID не выдаётся
  за владельца, существующий marker сохраняется. Нормальное
  завершение/отмена/unwind убирает marker, process exit оставляет его. Sweeper
  требует валидный PID и подтверждение его отсутствия; age — дополнительный
  порог, не основание считать активную загрузку брошенной. Неизвестные/старые
  partials без marker, ссылки, живой или повторно использованный PID сохраняются.
  Content staging также очищается только при подтверждённо завершившемся PID.
  Recovery читает CAS, а не staging: его очистка не освобождает journal leases.
  GC опубликованного CAS/runtime generations этим не реализован. Области и
  обязательные gates будущего collector описаны в
  [retention-contract.md](retention-contract.md).

## Проверки

`process_exit_recovers_each_publication_boundary_without_duplicate_links`
запускает отдельный процесс с файловой SQLite и завершает его через `exit(73)`
без destructors после `preparing`, staging/leases, publication intent, первого
файла, SQL completion и journal cleanup. Для staging/intent используется реальный
`install::execute`; остальные checkpoint fixtures вызывают те же storage/copy/
publish helpers. Повтор recovery не дублирует bindings.
Для реальных staging/intent checkpoints также проверены ownership markers,
удаление брошенного content staging на старте и успешный recovery без него.
`process_exit_followed_by_a_user_edit_keeps_the_journal_and_user_bytes` проверяет
сохранение изменённого файла и leases после рестарта.
`process_exit_allows_import_staging_cleanup_without_collecting_bytes_or_live_owners`
завершает другой процесс внутри реального CAS import после копирования/до
публикации. Cleanup удаляет только его scratch и сохраняет живой импорт,
старый unmarked файл и опубликованный объект с alias.

Это **не power-cut эксперимент**. Эти проверки выполнены на Linux; Windows/macOS
runtime, сетевые файловые системы и падение внутри syscall/SQLite commit не
проверены. Для более сильной гарантии нужны platform-specific порядок fsync
файлов/всех созданных директорий/SQL, проверка completed links и отдельные fault-
injection/power-cut испытания. Пока такой контракт не реализован, обещать его API
потребителям нельзя.
