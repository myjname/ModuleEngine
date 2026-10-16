# PropertySchema assets и сохранность Inspector после ревью 5e3f064

Работа начата от `5e3f0649d0b67bf41ddae330700385daa164f26c` в ветке `codex/property-defaults-inspector-keys`. Независимые воспроизведения: `outputs/independent-audit/four-p2-5e3f064/`; новые журналы и результаты: `outputs/remaining-authoring-fixes-5e3f064/`. Это продолжение аудита трёх конкретных ошибок, а не новая приёмка всего движка.

## Исходные ошибки и проверка до исправлений

`red-property-defaults.log`: реальный PNG используется только через `ModuleDescriptor.properties` / `PropertySchema.default`, но usage index возвращает ноль. Защита `ComponentSchema` из предыдущего этапа не покрывала этот другой механизм свойств.

`red-inspector-keys.log`: настоящая egui-кнопка Apply теряет буквальный JSON-ключ `future.key` и смешивает его с вложенным `future → key`. JSON допускает оба одновременно. Зарегистрированный dot-path override не является универсальным адресом произвольных JSON-ключей.

`red-inspector-inheritance-direct.log`: Apply без правок создаёт local aggregate на экземпляре prefab и мешает последующему наследованию. Его проверяет уже собранный baseline test executable. Первоначальный Cargo-запуск третьего теста попал на временно несовместимые параллельные правки caller API; его compile-error журнал тоже сохранён отдельно и не считается воспроизведением дефекта.

## PropertySchema и граница live IDs

Доступность свойств определяется тем же Core API, что runtime: effective modules, owner, requires_modules, requires_components и legacy attachments конкретного объекта. Затем asset service отдельно проверяет exact owner module pin. Global и Module не требуют объекта; Object и legacy Component разрешаются только для переданного множества реальных live IDs.

`AssetWorkspace::with_property_object_ids` передаёт snapshot IDs, а не создаёт мир. Editor берёт его у уже принятого runtime. `Runtime::world_source_matches` сверяет подключённый источник мира, ID, модуль и параметры создания без запуска фабрик. Если автор переключил мир, старые ID не выдаются за ID нового мира. Авторские scene IDs и ключи property store не подменяют runtime IDs.

Без известного множества ID потенциально применимые Object/legacy Component asset schemas дают явную ошибку контекста до файловой операции. Global/Module и проекты без таких scoped asset contracts продолжают работать. Библиотечный клиент, который знает реальное множество, должен передать его явно; пустое множество отличается от неизвестного.

Authored ссылки остаются редактируемыми. Ссылки, пришедшие только из installed default, учитываются отдельно с происхождением. Перенос ссылки-пути, Delete, Replace и Detach отклоняются до commit файлов, проекта, history или trash, если операция потребовала бы менять immutable schema. Stable `asset://ID` допускает Move с сохранением ID. Явный допустимый null подавляет default. Повторная проверка кандидата после Detach предотвращает восстановление удаляемой зависимости из default. Формат проекта, schema pins и installed schemas не изменяются ради операции.

Assets browser получает выбранный backend и тот же ID context, что файловые операции. Контекст участвует в актуальности фонового сканирования: результат другого backend или другого множества ID нельзя принять за текущий.

## Inspector: точные ключи и наследование

Внутренний обход JSON различает точный ключ объекта и индекс массива. Буквальный `future.key`, вложенный `future → key`, числовые ключи, пустые строки, `/`, `~`, массивы и null сохраняют исходную структуру. Persisted формат не меняется.

Dot override создаётся только для пути, однозначно представимого существующей грамматикой и принадлежащего нужному компоненту по longest-owner правилу. Если opaque правка требует local storage, переносится полный верхний aggregate с точным исходным ключом и соседними значениями.

Сравнение с эффективным prefab выполняется до обработки конфликта владельцев. Поэтому неизменённый унаследованный `foo.bar` не превращается в local aggregate только из-за отдельно прикреплённого компонента `foo.bar`. Реальная правка opaque поля по-прежнему материализует aggregate; его соседи и отдельный компонент остаются сохранны.

Каждый такой верхний aggregate копируется один раз за Apply. Обход большого opaque массива не копирует весь массив повторно для каждого его элемента.

## Проверки и ограничения

Проверенный code commit — `fb63b3ddae7cad52532761932855b755c61fb2c7`. Свежие результаты относятся к нему; старые количества тестов не используются. После него изменяются только документация и её source inventory.

| Проверка | Windows | Ubuntu/WSL2 |
| --- | --- | --- |
| Offline locked release workspace/all-targets | 605 passed, 0 failed, 9 ignored | 614 passed, 0 failed, 8 ignored |
| Отдельно выбранные native parent tests | 7 passed | 6 passed |
| fmt, strict all-targets Clippy, doc-tests, Editor/Player build | exit0 | exit0 |
| Свежий Full → Player, Full → Team → Player | passed | passed |
| Перенесённые Player, PATH без Cargo/Rust/DXC | 2 passed | 2 passed |
| Настоящее окно Editor, 90 stopped + 90 Play samples | WGPU/DX12, passed | WGPU/Vulkan X11 и Wayland, passed |
| Game-worker: slow Custom, lifecycle, fault latch | passed | passed |

Журналы: `windows-workspace-tests.log`, `linux-workspace-tests.log`, `windows-native-*.log`, `linux-native-*.log`, `windows-integration-*.log`, `linux-integration-*.log`. Сводные результаты: `windows-verification-summary.json`, `linux-verification-summary.json`, `linux-integration-all-summary.json`. Они проверяют собственные immutable snapshots каждого экспорта, hashes всех файлов пакетов, SDK identity и exact contract lock. Ранний экспорт не обязан совпадать с последним shared Cargo executable после последующих сборок; оба сохранённых snapshots проверены отдельно. Full — исходный проект, запущенный собранным Editor; исполняемый Editor поставляется с Team.

Native tests исполняются по фактическим test executable из свежего Cargo JSON, с нормальным loader environment. Stop/reuse на Linux: `root_artifacts_built` 1 → 1 → 1; на Windows 1 → 2 → 2 с явно записанным появлением environment key `=C:` перед Scene. Последующий Stop/Game использует fresh artifact; состояние и worker PID независимы. Три вызова Cargo для freshness proof не означают три root compilation. Windows tool matrix включает 12 вариантов Cargo/Rustc имени/пути и сохранение настоящей ошибки Rust.

Все три первоначальных дефекта имеют regression tests; Inspector проверяется через реальную egui-кнопку Apply с save/reopen и strict profile/lock validation. PropertySchema tests используют настоящие PNG и файловые операции. Для Object/legacy Component custom Runtime выдаёт ID17/29, отличные от authored Scene1/2; счётчик фабрики подтверждает отсутствие повторного создания мира при сканировании.

Неудачные промежуточные попытки сохранены. Первый focused project run имел 30 passed / 1 failed из-за некорректного fixture: attachment29 не имел доступного свойства. Добавлено независимое Boolean-свойство; gated AssetReference остаётся доступен только object17. Исправленный тест прошёл в обоих полных прогонах. Заметки `windows-verification-attempt-note.json` и `linux-collector-attempt-note.json` описывают исправленные предположения сборщиков отчётов, без изменений продукта. Первоначальные журналы сохранены; итоговые проверки выполнялись заново.

## Оконные измерения и оставшиеся границы

Windows на RTX 4080 SUPER: prewarmed Editor завершился за 18,74 с, p95 обработчика — 2,33/2,59 мс, интервал callback — 7,89/7,46 мс. Focus=false, visibility=null: это не ручная foreground input-to-display проверка.

Linux — WSLg software Vulkan `llvmpipe (LLVM 20.1.2)`. X11/Wayland завершились за 13,19/22,32 с, оба complete=true, measurements_valid=true, errors=[]. p95 обработчика X11 — 9,18/10,10 мс, callback — 25,55/27,23 мс; Wayland — 10,48/10,58 мс и 107,34/69,27 мс. У X11 focus/visible подтверждены во всех 90 samples каждой фазы; Wayland focus подтверждён, visibility неизвестна. Эти данные не подтверждают 60 FPS.

Linux fixture явно лишён audio cues только в свежем Full из-за текущего unsupported native WAV. Удалённые cues, SHA capability source и оригинального JSON записаны в `linux-integration-audio-fixture.json`. Авторские данные и scheduled Rust неизменны. Compiler-less Player verify выполняет offscreen960×540 с отключёнными native services; он не проверяет звук, реальное presentation window или пользовательский ввод.

В этот этап не входят cold Editor within45s, строгие Player60FPS, native Linux audio/dialogs, аппаратная Linux-сертификация и ручная foreground input-to-display QA. Их прежний открытый статус не меняется от успешных authoring regression tests.

## Сборка и сохранность

Новая Windows-сборка: `outputs/ModuleEngine-authoring-fixed.exe`, SHA256 `D9DEB3C70D36432D2BD0C9AF39C8D4A910E2ABF2836D832F05E902EFC8AC195C`. Запуск:

```powershell
.\outputs\ModuleEngine-authoring-fixed.exe game-project-v4.json
```

Прежние `outputs/ModuleEngine.exe` (SHA2477792D…) и `outputs/ModuleEngine-review-fixed.exe` (SHAA5E4F9E8…) сохранены. Девять защищённых файлов и ignored metadata сверены с `stage0-baseline.json`. Авторские проекты, SDK metadata, binding и personal layouts не заменялись. Текущих native-процессов этих проверок после завершения не осталось; сторонние процессы не останавливались.

Работа выполнена в одном Git checkout. Full/Team/Player папки — export acceptance fixtures, а не дополнительные development copies; dependency targets используются повторно. ОС, драйверы, security settings, ABI/SDK identity, release profiles, GUI/backend policy, watchdog и IPC limits не менялись. Использованы уже установленные инструменты.

Повтор базовых проверок после подключения существующего окружения инструментов:

```text
cargo fmt --all -- --check
cargo clippy --offline --locked --release --workspace --all-targets -- -D warnings
cargo test --offline --locked --release --workspace --all-targets -- --test-threads=1
cargo test --offline --locked --release --workspace --doc
cargo build --offline --locked --release -p module-engine-editor -p module-engine-player
```

Для native сценариев нужны реальные Rust/Cargo/DXC, Vulkan и выбранная оконная система. Точные executable, команды, hashes и normal-loader environment сохранены в текущих evidence. Документация API: [AssetWorkspace](asset-workspace-api.md).
