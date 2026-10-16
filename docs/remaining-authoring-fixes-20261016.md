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

## Проверки и ограничения

Результаты свежих Windows/Linux проверок будут добавлены после завершения реализации. Старые количества тестов и старые сборки не считаются проверкой этого исходного кода. Оба существующих EXE и девять защищённых файлов сверяются с `stage0-baseline.json`.

В этот этап не входят cold Editor within45s, строгие Player60FPS, native Linux audio/dialogs, аппаратная Linux-сертификация и ручная foreground input-to-display QA. Их прежний открытый статус не меняется от успешных authoring regression tests.
