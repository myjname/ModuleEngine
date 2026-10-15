# Формат проекта 0.4

GraphDocument — UTF-8 JSON. Текущая schema_version — 3; версии 1/2 читаются с default новыми полями. Загрузка мигрирует документ в памяти; сохранение существующего файла оставляет точную резервную копию. Неизвестные версии отклоняются.

## Документ

| Поле | Содержание |
| --- | --- |
| schema_version | Версия формата документа. |
| name | Имя проекта. |
| root | Корневой Graph. |
| custom_sources | Карта source_id → полный текст Rust. |
| shader_sources | Карта source_id → полный текст HLSL. |
| asset_manifest | null или манифест ресурсов с относительными путями и SHA-256. |
| module_versions | Карта ID модуля → точная версия установленной реализации. Несовпадение или отсутствие объявленного модуля даёт ошибку; это implementation pin, а не независимая версия ABI. Автоматический подбор совместимых версий отсутствует. |
| engine / project / properties | Авторские module/exposure настройки, разрешённые Project controls и active/inactive PropertyStore. |
| game | null или GameProject: entry_scene, scenes, prefabs, execution и audio_cues. Старые графовые проекты читаются с game:null. |

У обычного Graph есть массивы nodes и edges. Node содержит id, module, label, position:[x,y], params и необязательный subgraph. Идентификатор узла уникален внутри своего Graph; вложенные области имеют собственные ID. Edge задаёт from_node/from_port/to_node/to_port. Геометрия редактора не задаёт порядок legacy/render исполнения: его определяют связи. Отдельный игровой ExecutionGraph задаёт stages/nodes в явном порядке; его after и typed resource bindings проверяются относительно этого порядка, а позиции canvas сохраняются отдельно.

Неизвестные поля фиксированных объектов документа, графа, узла и связи отклоняются, чтобы опечатки не исчезали при пересохранении. params хранит произвольный JSON; исполнитель конкретного модуля ограничивает его допустимые ключи.

## Настройки и поля

engine содержит initialized, enabled_modules и exposure {parameters, variants}. Он принадлежит Engine. project содержит parameters, variants и rules; это разрешённые значения/условия Project. properties содержит active, inactive и detached_components. У active/inactive есть global, modules, objects и components; присутствие component map означает участие объекта. Inactive данные не передаются runtime. Подробнее — [настройки](settings.md).

Личная раскладка областей не входит в GraphDocument: она сохраняется в .user/<profile>/<workspace>.json. params.code_layout/code_layout_keys остаются авторской раскладкой самого Custom графа.

## Контракты и проверка

Реестр ModuleRegistry содержит ModuleDescriptor: ID, название, описание, входы и выходы. PortDescriptor задаёт name, data_type и required. Совместимость корневых портов основана на точном совпадении типа. Один вход получает не более одной связи; обязательные входы должны быть подключены. Проверяются неизвестные модули/порты, дубликаты ID, циклы, типы и вложенные интерфейсы. Неизвестный модуль остаётся в документе с диагностикой.

Составной модуль объявляет внешние порты и внутренние interface.input/interface.output. Их params задают port и type. Интерфейсы должны совпадать с внешним контрактом. Vulkan дополнительно проверяет формат, semantic, extent, samples, access и реализацию шейдера. Типизированная связь сама по себе не доказывает наличие исполнителя для произвольной архитектуры.

## Legacy и GPU маршруты

Сохранённый legacy Runtime требует ровно один viewport.output. Его frame должен приходить от зарегистрированного renderer; scene рендерера — от scene.extract, а world адаптера — от зарегистрированного WorldProvider. Встроенные миры: world.objects и world.ecs. Внешнее API позволяет добавить третий мир или другой renderer через исходники. GameHost текущего Player проверяет этот внешний blueprint и требует установленный render.vulkan с вложенным графом; его игровая симуляция выполняется отдельно через game.execution.

Scalar-граф поддерживает scalar.value, clock.time и custom.rust. custom.rust содержит params.source_id и необязательные params.code_layout/code_layout_keys. custom.rust.update использует параметр source_id и выдаёт команды для подключённых входов мира и рендерера. Один source_id не может одновременно использоваться узлами speed и update; конфликт режимов даёт ошибку. Неподключённая ветвь не исполняется; предупреждения сообщают о таких фрагментах.

render.software3d исполняет только установленный внутренний маршрут input → render.rasterize → output. render.vulkan исполняет поддержанные gpu.* проходы во вложенном subgraph. Полный контракт проходов и пример params.manifest — в [GPU/HLSL](gpu-hlsl.md).

## Авторские сцены и общее исполнение

game.scenes содержит именованные AuthoredScene с AuthoredObject: object (ID/parent/local Transform/color), prefab, components, overrides и references. game.prefabs содержит defaults цвета/компонентов; field overrides имеют dot paths, например health.maximum. Play использует разрешённую копию с активными компонентами, не изменяя источник. Полный формат — в [scenes.md](scenes.md).

game.execution содержит resources {name,data_type,external,initial} и stages {name,nodes}. SystemNode содержит id, module, params, bindings и after. Здесь действует SystemRegistry с настоящими ExecutableSystem factories и schema состояния; этапы исполняются в сохранённом порядке. В новом game-project.json отсутствуют legacy scalar/value/clock контроллеры: поля акторов и игровые системы задают движение. Примеры контрактов — [module-execution.md](module-execution.md), работу черновика UI описывает [editor-game.md](editor-game.md).

## Сохранение и совместимость

Файловый API может сохранить синтаксически корректный документ с неисполняемым графом или ошибочным Rust для последующего исправления; сохранение не обещает исполнимость. Обычный Graph/File редактирует сам документ. Игровой graphical execution editor имеет отдельный неполный черновик: он не попадает в Save/Export до успешного Apply. Неверная сцена отклоняется авторским UI целиком; ошибочный Rust-текст сохраняется отдельно от последнего рабочего runtime.

В schema 1 отсутствующие shader_sources, module_versions и asset_manifest, а в schema 1/2 engine/project/properties принимают значения по умолчанию. Миграция не переводит программный renderer в Vulkan автоматически и не меняет архитектуру старого проекта. Резервная копия при явном сохранении позволяет вернуться к исходным байтам.

При загрузке schema1/2 известные first-party pins 0.1.0/0.2.0, а при schema3 first-party pin 0.3.0 переводятся в текущую установленную версию 0.4.0 по явному списку известных модулей. Файл при этом не переписывается. Произвольные third-party ID и другие несовместимые версии сохраняются. Сохранение/экспорт добавляет только отсутствующие версии, включая feature-модули; отдельные package/SDK версии также должны быть совместимыми.

Новая несовместимая версия требует явной миграции или нового ID. Resolver сторонних версий и динамическая установка плагинов не реализованы. Team отдельно проверяет frozen blueprint [поставки](distribution.md), поэтому сохранение ошибочного gameplay текста не разрешает переписать архитектуру.

