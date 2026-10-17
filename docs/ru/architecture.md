# Архитектура 0.4

[English](../architecture.md) | [Русский](architecture.md) | [Документация](README.md)

Проект хранит архитектуру, авторские сцены и исходники в редактируемом JSON. `ModuleRegistry` описывает корневые world/render-модули, `SystemRegistry` устанавливает исполняемые игровые подсистемы. Нейтральный снимок отделяет хранение мира от рендера, а owned команды и состояние отделяют Rust Custom от конкретных хранилищ и Vulkan.

## Пакеты и границы

| Пакет | Назначение |
| --- | --- |
| module-engine-core | GraphDocument, реестры, Object/ECS, WorldObject/команды, SceneSnapshot/Material, авторские сцены и шаблоны, SystemExecutor и GameSession. Не зависит от Vulkan, UI, импорта изображений или rustc. |
| module-engine-content | Статический glTF/GLB, PNG/JPEG, декодированный кеш, материал/геометрия и зависимости ресурсов выбранного проекта. |
| module-engine-vulkan | GPU-граф raster/compute, контракты ресурсов, HLSL/DXC и проверка SPIR-V, реальные текстурированные Forward/Deferred проходы. |
| module-engine-custom | Частичные визуальные представления Rust, правки исходных диапазонов и сохранение opaque-текста. |
| module-engine-script | Сборка Rust worker, постоянный процесс, ограниченный JSON-lines IPC и явно объявленное состояние с validate/commit/reject. |
| module-engine-project | История документа, контрольные версии, diff/merge, миграция, asset manifests и экспорт зависимостей. |
| module-engine-editor | Нативный egui/eframe UI, GameHost, редактор сцен/исполнения, политики Full/Team и отдельный Player-режим. |

## Два маршрута исполнения

Сохранённый демонстрационный маршрут `Runtime` обслуживает `World → scene.extract → зарегистрированный renderer → viewport.output`. Дополнительные входы принимают WorldCommands/RenderCommands; необязательный Scalar-граф управляет скоростью. Он обслуживает один viewport и ограниченный набор маршрутов. Обычный корневой Node descriptor сам по себе не создаёт поведение произвольной подсистемы. Этот маршрут используется старым `project.json`, окнами 3D Preview и Object Tree/Inspector.

Игровой маршрут использует `GraphDocument.game`: GameProject, сцены/шаблоны и ExecutionGraph. `SystemRegistry` регистрирует descriptor вместе с фабрикой `ExecutableSystem`. `SystemExecutor` создаёт экземпляры и исполняет сохранённые стадии/узлы в указанном порядке. Типы ресурсов, обязательные порты, `requires` и `after` проверяются до выполнения; в executor не добавляется ветвь на имя каждого нового модуля. `GameSession` владеет Play-копией, fixed clock 1/60 и каноническим `GameState.world: EcsWorld`. В ECS-маршруте второго провайдера нет; Object и явно подключённые сторонние провайдеры служат совместимыми проекциями. `module-engine-host` соединяет session, контент, persistent Custom, зарегистрированный `FrameRenderer` и явно запрошенные сервисы. Он общий для окна Game / Scene и Windows Player; от редактора, Vulkan и игровых пакетов общий crate не зависит.

Игровой порядок расположен в `game.execution`, а не выводится из позиций узлов или legacy Scalar-цепочки. В новом `game-project.json` скорость акторов задают компоненты сцены; прежний scalar controller не является его игровым циклом. Общий API native-систем не означает, что все корневые Graph-модули уже выполняются универсальным планировщиком. Приложение регистрирует Vulkan через его адаптер; общий host проверен также с независимым CPU-рендером и в headless-режиме. Другие GPU API и динамический native binary ABI пока не реализованы. См. [общий host](shared-game-host.md).

## Мир и контент

В старом Runtime Object хранит реальную рекурсивную иерархию, ECS — независимые таблицы компонентов. В игровом маршруте изменяемые transform, parent, color, ссылки и динамические компоненты принадлежат одному ECS. Native-системы используют запросы по колонкам, а DTO объектов извлекаются для UI, IPC и проверок; [контракт и время жизни](ecs-world.md). `from_objects` создаёт оба хранилища из разрешённой авторской сцены без demo angular velocities. Их команды и снимки эквивалентны для одинаковых данных. Renderer получает вершины/UV, матрицы, цвет и Material; он не запрашивает ECS или объектное хранилище. Матрицы — row-major, вектор — столбец.

ContentLibrary присоединяет импортированные примитивы по стабильному object ID. Локальные матрицы glTF компонуются с матрицей объекта; TextureImage разделяется между снимками через Arc. Импорт и экспорт ограничены корнем выбранного проекта/пакета. Непрозрачный base-color материал исполняется на GPU; skeletons, transparency, полноценный PBR, mipmaps и streaming ещё не реализованы.

## Авторство и сохранённые данные

EngineConfiguration задаёт активные feature-модули, exposed ranges и готовые варианты рендера. ProjectConfiguration хранит разрешённые значения/условия. Legacy PropertySchema фильтрует поля по фактическому участию active module/component. В Game / Scene схемы компонентов регистрируются отдельно от потребляющих их систем. Состав вычисляется по выбранной композиции, включая явно объявленные Custom `params.components`; mesh/material активируются подключённым рендером. Поля, defaults, ограничения и ссылки берутся из общей схемы, а не из случайного prefab. Неактивные авторские значения остаются у автора; runtime и новые stripped Team/Player данные их не получают.

Full Engine требует полного совместимого native source manifest. Team всегда Project-only: blueprint фиксирует архитектуру, игровой ExecutionGraph, bindings/source IDs, shaders и версии; сцены, активные шаблоны и разрешённый gameplay Rust остаются игровыми данными. Изменённые Full sources остаются editable. `.sdk` Team нужен для compiled Custom; package tools/assets root отделён от SDK source root. Самостоятельный Player имеет готовые workers и `.spirv`, не требует SDK/Rust/DXC. Это политики поставки, а не sandbox произвольного native/Rust кода.

Личные Engine/Project dock layouts хранятся вне игры. Начальный Game layout создаётся только при отсутствии сохранённого личного layout; существующая раскладка сохраняется. В ней Game/Scene можно выбрать вручную из списка окон. Legacy Preview области имеют независимые камеры/выбор при общем шаге Runtime. Игровой scene viewport показывает отдельную Play-копию. Pause сохраняет runtime, Stop его уничтожает и возвращает авторские данные. Граф исполнения редактируется локальным черновиком; Apply требует Stop и проверяет целый кандидат. Координаты его canvas хранятся отдельно в `.user/full/game-systems-positions.json`.

## GPU и измерения

`render.vulkan` имеет внешний контракт Scene3D → Frame3D. Его subgraph соединяет реальную геометрию, Forward/G-buffer/Deferred, SSAO, tone mapping, FXAA и encoding. ResourceSpec задаёт format, semantic, extent, samples и access; DXC/SPIR-V reflection проверяет bindings и реальные image formats. Custom scene raster может явно включить material ABI через `scene_materials`.

Игровой renderer работает offscreen с синхронным readback; egui выводит полученные кадры через WGPU/DX12 на Windows и WGPU/Vulkan на Linux. Игровой renderer остаётся отдельным модулем Vulkan; backend интерфейса не задаёт формат игрового графа. Glow доступен только как явно выбранный диагностический режим. У игрового renderer нет Vulkan swapchain и нескольких кадров в полёте. Изображения проходов, vertex/readback buffers и GPU-атлас используются повторно; CPU чтение предпочитает cached coherent память. Player возвращает только RGBA8, без диагностического чтения depth/normal ownership каждого кадра. Это действующий путь исполнения и измерения текущей игры; он не является оценкой предельной скорости любого будущего движка.

Renderer-only benchmark измеряет проходы и readback. GameHost check включает симуляцию, Rust IPC, звук, контент и рендер. Настоящий Player window benchmark дополнительно включает texture conversion/upload, HUD, вывод через egui/WGPU, vsync и планирование ОС. Их результаты и критерии различаются: [GPU/HLSL](gpu-hlsl.md), [game verification](game-verification.md), [Player](player.md). Числа нового полного оконного замера публикуются после измерения, а не выводятся из renderer-only FPS.

## Кандидаты, состояние и отказ

Rust компилируется в фоне. Совместимая версия переносит явно объявленное состояние; несовместимая остаётся кандидатом до reset или migration JSON. Legacy Preview и игровые экземпляры имеют отдельное owned состояние. Persistent worker устраняет запуск процесса каждый tick; после timeout/panic/невалидного пакета он перезапускается с последним принятым JSON. Globals не являются контрактом сохранения/миграции и не переносимы между версиями.

Игровой executor выполняет initialize/step всех систем, проверяет все staged outputs, затем host-транзакция проверяет и применяет пакет команд мира, и только после этого выполняются system commit hooks. Ошибка вызывает rollback у участвовавших систем и host-транзакции, возвращает world/resources/events/owned state. GameSession сохраняет один ECS между fixed ticks и прямыми командами. Если выбран совместимый внешний WorldProvider, его экземпляр также сохраняется; новый создаётся при входе в сцену, переходе или Restart. Provider обязан выполнять validate_commands без побочных эффектов, а apply_commands — атомарно. Для отката после публикации host применяет обратный portable пакет; скрытое состояние и внешние native эффекты требуют собственных транзакционных hooks. Запись файла не становится автоматически обратимой, а ошибка rollback явно сообщается. После перехода сцены сценовые системы создаются заново; persistent игрок переносит компоненты при локальных transform/references назначения.

Применение совместимого render graph/HLSL или разрешённых Project controls сохраняет живую игру. Изменение native ExecutionGraph требует Stop; обычный native world/renderer/UI исходник требует сборки. Legacy Runtime отдельно переносит провайдер при совпадении ID/типа/параметров мира и очищает несовместимые renderer controls. Эти механизмы не следует смешивать с сохранением игрового прогресса на диск.

## История и объединение

Undo/redo и checkpoints сохраняют GraphDocument, включая сцены, шаблоны, графы, исходники и параметры. Они не сохраняют живые объекты, таймеры, GPU-ресурсы или Custom runtime state. Возврат документа не перематывает Play.

Merge нормализует обычные Graph nodes по ID и edges по входу. Сценовые arrays объектов и arrays стадий/узлов ExecutionGraph пока не имеют поэлементного ID-aware merge: пересекающиеся изменения такого массива дают конфликт целиком с base/ours/theirs. При конфликте preview сохраняет базу. Это граница текущего алгоритма, а не автоматическое объединение двух сцен.

Подробнее: [формат](graph-format.md), [исполнение модулей](module-execution.md), [сцены](scenes.md), [редактор игры](editor-game.md), [контент](content.md), [Custom state](custom-state.md), [история](history-assets.md).

Точные контракты формата 4, явная публикация профилей, миграция в копию и отдельные feature-selected Player описаны в [Engine profiles](engine-profiles.md). Общий Host не зависит от редактора; Player и редактор устанавливают одинаковые source-контракты через composition.
