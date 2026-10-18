# Повторное использование Custom Rust

[English](../custom-program-store.md) | [Русский](custom-program-store.md) | [Документация](README.md)

`CompiledProgramStore` хранит неизменяемый скомпилированный код между подготовками Game, Scene и Rust hot apply. Цель — не собирать тот же worker заново после Stop, сохраняя независимость игровых запусков. Это отдельный слой над обычными Cargo dependencies; постоянное игровое состояние по-прежнему принадлежит host, а не кэшу.

В обычном Editor ничего включать не нужно: запускайте Play, нажимайте Stop и запускайте снова. При неизменных подтверждённых входах готовый код используется повторно, а игровое состояние начинается заново. Подготовка всё равно проверяет файлы и вызывает Cargo, поэтому повторный запуск не обязан быть мгновенным. После полного закрытия Editor память store освобождается; следующий процесс может переиспользовать проверенные зависимости SDK на диске, но заново готовит root программы.

## Владение и подключение

`GameWorkbench` владеет одной `Arc<CompiledProgramStore>`. Stop освобождает игровой host/actor и сохраняет store. Каждый успешный запрос компилятора, включая cache hit, создаёт новый `ScriptInstance`: отдельные state, канал и worker. `Scene` и `Game` не получают общий mutable instance. Сам процесс worker запускается при первом вызове экземпляра. При совместимом hot apply переносом состояния управляет существующий host contract, описанный в [custom-state.md](custom-state.md).

Для собственного host создайте `Arc<CompiledProgramStore>` и передайте его в `ScriptCompiler::with_program_store` либо `GameHost::build_cancellable_with_store`. Прежние constructors остаются доступны. CLI-обёртка редактора создаёт локальный store; зависимости SDK могут использовать тот же подтверждённый дисковый namespace в следующем процессе.

`CompiledScript::clone` сохраняет прежнюю семантику общего instance. Для независимого запуска нельзя заменять новый запрос store обычным clone. Запись store содержит только metadata, build recipe и artifact leases, а не живой `ScriptInstance`.

## Что проверяется

До повторного использования проверяются по содержимому исходник и режим Custom, Core/SDK, Cargo locks, фактический dependency/feature graph, поддержанная native toolchain, target, конфигурация Cargo и выбранные compiler/link inputs. Root key также учитывает эффективное окружение и рабочие пути. Отсутствие значимого config/toolchain файла тоже является частью проверки: появление такого файла должно изменить решение. Время изменения файла не служит доказательством неизменности.

Успешная запись содержит фактические Cargo JSON artifacts, dep-info, generated worker inputs и executable. Обычный warm hit сначала сверяет доказательство, затем запускает Cargo `--offline --locked` в сохранённом собственном workspace. Требуется реальный `compiler-artifact` с `fresh=true` для root binary; счётчик не имитирует ответ Cargo. После этого входы проверяются ещё раз.

Для запроса, который действительно ожидал ту же выполняющуюся компиляцию, предусмотрен более узкий путь. После успешной публикации такой запрос может один раз использовать доказательство завершённой подготовки без повторного Cargo и построения post-build seal. При этом заново разрешается контекст, хешируются текущие входы, проверяется SDK namespace под его lease, а также generated inputs и байты executable. После этих проверок должны по-прежнему совпадать конкретные resident program, build, seal и SDK target. Неуспешная или отменённая подготовка, отмена ожидающего запроса, изменение входов или products, вытеснение и замена записи не дают права на такую передачу. Обычный последующий вызов, включая подготовку после Stop, не имеет такого свидетельства и сохраняет полный путь с Cargo. Общим остаётся скомпилированный код; каждый возвращённый экземпляр имеет независимые состояние и workers.

Изменение во время подготовки отклоняет кандидата, сохраняя прежний работающий код. Это проверки до/после, а не атомарный снимок файловой системы и не гарантия против произвольных внешних ABA-изменений. Проверка при передаче не перечитывает произвольные служебные данные Cargo, которые не используются при запуске уже скомпилированного executable.

Отдельная persisted SDK receipt проверяет outputs до того, как Cargo сможет использовать старый dependency namespace. Она охватывает SDK products, build-script outputs и control files, fingerprints и compiler-query cache. Это требуется и для нового исходника, и для нового процесса редактора: доказательство исходников не доказывает сохранность старого `.rlib`. Межпроцессная namespace lease охватывает проверку, Cargo, повторную проверку и публикацию receipt. Отсутствующая или неверная receipt при существующих products требует нового SDK generation; marker каталога сам по себе недостаточен.

Каталог SDK использует короткий locator `sdk-v3/<16 hex>`, чтобы не расходовать Windows linker path budget. Идентичность остаётся полным 64-символьным SHA-256 в marker, receipt и ключах памяти. При совпадении коротких имён разных ключей чужой namespace не принимается и не перезаписывается: применяется новый собственный generation. Очень глубокий выбранный target всё ещё может упереться в ограничения native linker; используйте короткий абсолютный `CARGO_TARGET_DIR`.

Неизвестный ранее `include!`/`include_str!`/`include_bytes!` может потребовать первого uncached build, чтобы получить реальные зависимости из dep-info. Следующая подготовка уже проверяет эти файлы заранее. Новый root generation не заимствует потенциально устаревшие root products. Если не подтверждены SDK inputs, используется новый собственный SDK generation. Повреждённый или отсутствующий executable не считается hit. Чужая или неподтверждённая директория кэша не принимается во владение и не удаляется.

## Поддержанная область доказательства

Это консервативно проверенный native build subset, а не поддержка произвольного Cargo workspace или герметичная копия ОС. Registry-зависимости разрешаются только для зафиксированного проверенного набора: дерево исходников сверяется с digest, полученным из `.crate` archive после проверки его checksum по worker lock. Изменения audited dependency list требуют повторной проверки и обновления `freshness/audited_dependencies.rs`.

Поддержаны штатные Windows MSVC и Linux x86_64 GNU/GCC linker routes, которые подтверждает native resolver. Hash/inventory включают выбранные compiler/linker files, native libraries и значимые каталоги поиска в их порядке. Неизвестные wrappers, custom linker/target flags, неподтверждённые build scripts/proc macros, native FFI inputs и другие неоднозначные настройки отключают reuse. Обычная компиляция при этом сохраняется, но использует новый собственный generation, а не сомнительный старый Cargo cache.

Для reuse Cargo home должен быть известен как абсолютный путь. Неопределённый default home и относительный `CARGO_HOME` не считаются доказанными: иначе Cargo мог бы прочитать config, который не вошёл в snapshot. В этих случаях сохраняется обычная компиляция без повторного использования. Само окружение компилятора не переписывается ради прохождения проверки.

Для MSVC reuse поддерживает обычный COM/registry discovery и developer shell с явным `VSCMD_ARG_TGT_ARCH`. Legacy shell без target arch и discovery, требующий fallback `vswhere.exe`, остаются uncached: используемая библиотека запускает там собственные неограниченные subprocess, которые наш resolver не принимает. Ограничение действует только в копии окружения discovery; настоящее окружение Cargo/compiler сохраняется. Это не гарантия ограниченного времени всех системных COM/registry calls.

Для обновления registry digests используется `scripts/generate-audited-custom-dependencies.ps1` в PowerShell 7.3+. Передайте `-LockPath` настоящего generated Custom worker, `-CargoHome` с локальными `.crate` archives и отдельный `-EvidenceOutput`. Скрипт не скачивает пакеты и не запускает Cargo. Он проверяет archive checksum по lock, безопасные regular-file paths и canonical tree framing, затем пишет Rust table и JSON receipt. Workspace lock целиком для этой операции не подходит. Изменение ожидаемого числа пакетов через `-ExpectedPackageCount` не заменяет ручную проверку нового набора зависимостей, build scripts и proc macros. Проверка текущего генератора воспроизвела все 23 package digests без изменений; файл результата хранится отдельно от production table.

Для штатного DXC окружения Linux разрешён узкий случай `LD_LIBRARY_PATH`: один canonical directory, ровно два обычных файла `libdxcompiler.so` и `libdxil.so`, без symlinks, вложенных директорий и дополнительных entries. Путь, содержимое и membership входят в проверку. Произвольные loader overrides, в частности `LD_PRELOAD`/`LD_AUDIT`, не получают такого исключения. Окружение не очищается ради искусственного cache hit.

Linux loader policy применяется на Linux. Windows MSVC loader не использует `LD_LIBRARY_PATH` и `DYLD_*`; их наличие само по себе не запрещает Windows reuse. Эффективное окружение root по-прежнему проверяется целиком, включая значения, которые читает пользовательский `env!`/`option_env!`. Cargo добавляет собственные loader directories при запуске тестов. Runtime cache assertions Linux запускаются отдельным test child с явно распознанными Cargo additions; production environment при этом не переписывается. Workbench cache regression помечен explicit native acceptance и запускается напрямую из compiled test binary в обычном native environment, отдельно от общего Cargo suite.

Изменение GUI-переменных вроде DISPLAY не должно пересобирать весь SDK: dependency namespace отделён от полного root environment. При этом новый root контекст проверяется отдельно. Cross-target Custom по-прежнему отклоняется: executable должен быть родным для текущего host.

## Лимиты и освобождение

По умолчанию: 16 записей, 256 MiB runnable executable bytes, четыре одновременно хранимых SDK contexts и две одновременно допущенные подготовки. Admission охватывает tool probes, metadata, hashing и Cargo, а не только сам build. Одинаковые одновременные запросы объединяются; ожидание проверяет cancellation. Лимиты настраиваются через `ProgramStoreLimits` при создании store.

LRU удаляет записи сверх лимита. Активные экземпляры и экспорт сохраняют независимые artifact leases: удаление записи не удаляет используемый executable. Последний владелец передаёт cleanup только подтверждённые собственные продукты. Отменённая, ошибочная или изменившаяся во время build подготовка не публикует cache entry.

Лимит 256 MiB относится к executable bytes записей store. Общие Cargo dependencies на диске, удерживаемые активными экземплярами artifacts и служебные build products не входят в этот показатель. Это не общий лимит размера target directory. Механизм не запускает глобальный `cargo clean` и не исправляет freshness изменением timestamps SDK.

## Диагностика и проверки

`stats()` возвращает атомарный snapshot без файлового IO, чтения исходников и захвата build mutex. `program_hits`, `cache_misses`, `cargo_build_invocations`, root/dependency built/fresh, eviction/coalescing, resident limits, причины отказов и время proof/build позволяют отличить повторное использование от настоящей пересборки. Snapshot не включает значения environment или тексты Custom; обычная диагностика компилятора сохраняет свой прежний формат. `root_artifacts_built` считает обычный worker binary, а не его build script.

Проверки runtime покрывают независимость экземпляров, реальные fresh events, cancellation/coalescing, bounded eviction/leases и повреждение products. Production Workbench regression отдельно проверяет Game → Scene → Stop → Game, исходный state/time, новый PID и неизменность авторского документа. Native Editor benchmark проверяет весь пользовательский маршрут с прежним общим watchdog 45 секунд; счётчики store добавлены к отчёту без ослабления его критериев. Результаты конкретных запусков и известные ограничения находятся в [review-fixes-20261016.md](review-fixes-20261016.md).
