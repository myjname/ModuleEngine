# Добавление модуля

Native расширения 0.4 компилируются вместе с исходным проектом. Для старого world/render графа используйте ModuleRegistry и его фабрики; для общих игровых подсистем — SystemRegistry с настоящими ExecutableSystem factories. Один descriptor обычного Graph позволяет показать узел, но не создаёт его поведение. Программируемые HLSL/Rust кандидаты имеют отдельный hot apply без native rebuild.

## Общая исполняемая подсистема

Реализуйте ExecutableSystem::step и при необходимости initialize/stop/validate_tick/commit_tick/rollback_tick. Зарегистрируйте SystemDescriptor вместе с фабрикой через SystemRegistry::register. Descriptor задаёт state_schema, typed ports, предшествующие requires и компоненты, потребляемые системой. Добавьте экземпляр SystemNode в game.execution с явными bindings/after и авторским порядком стадий. Сам executor не проверяет имя нового модуля специальной ветвью.

Состояние экземпляра находится в переданном JSON, а доступ к world/input/events проверяется соответствующими helpers SystemContext. Другие данные используют typed именованные ResourceStore envelopes. Подключайте registry к GameSession::new_with_registry; Player/редактор получают свои native регистрации через composition root. Для третьего самостоятельного хранилища авторской игры предусмотрен GameSession::new_with_world_factory. Компилируемый пример, lifecycle и границы транзакции — [module-execution.md](module-execution.md); формат сцены — [scenes.md](scenes.md).

## Третий мир

Готовый самостоятельный пример — examples/third_world.rs:

~~~powershell
cargo run --locked -p module-engine-core --example third_world
~~~

Пример регистрирует world.pyramid и заменяет module узла мира в legacy demo_document. Renderer получает тот же SceneSnapshot: новая модель хранения не требует правки программного рендерера. Для сохраняемой игры нужен редактируемый world store с портативными WorldObject/commands; один read-only demo snapshot ещё не является авторской GameSession.

Для своего мира:

1. Реализуйте WorldProvider::step(dt, speed) и snapshot(). Провайдеру принадлежат состояние и модель хранения; renderer получает только снимок.
2. Создайте descriptor через world_descriptor(id, label, description) и вызовите registry.register_world(descriptor, factory). Фабрика получает node.params.
3. Замените module существующего World-узла, сохранив его внешние связи.
4. Чтобы поддержать Custom update, реализуйте чтение WorldObject и транзакционное применение WorldCommand. Реализация по умолчанию явно отказывает в редактировании, сохраняя поддержку рендера.
5. Назначьте версию реализации через set_module_version при независимом выпуске и проверьте старые проекты.

Стандартная граница: optional speed:Scalar/commands:WorldCommands → world:World. ID объектов должны быть устойчивыми; их локальные Transform и parent сохраняют общий смысл для обеих моделей мира.

## Другой renderer

FrameRenderer::render принимает SceneSnapshot, Camera, размеры и RenderOverrides, возвращает Frame. backend_name описывает активный backend; validate_overrides проверяет поддержанные команды. Frame — RGBA8 изображение с измеренными RenderStats.

ModuleRegistry::register_renderer(descriptor, factory) принимает фабрику Fn(&Node, &GraphDocument) → Result<Box<dyn FrameRenderer>, String>. Descriptor обязан иметь обязательный scene:Scene3D и единственный frame:Frame3D output. Optional commands:RenderCommands можно объявить для управления. Фабрика получает вложенный graph и исходники целого документа.

Editor composition root находится в crates/editor/src/host.rs. Зарегистрируйте там renderer и descriptors внутренних проходов через register_module. CLI, UI и exporter должны получать один и тот же реестр. Обновите пример проекта и проверки; несовместимый старый module_versions pin нельзя молча менять.

## Новый GPU-проход

Для собственного эффекта обычно достаточно gpu.custom, без нового Rust-пакета:

1. Скопируйте близкий shipped HLSL-шаблон в shader_sources под новым source_id.
2. Объявите PassManifest: stage, entry, inputs/outputs, parameters и при необходимости vertex_entry/raster_mode/workgroup.
3. Подключите только совпадающие format/semantic/extent/samples. Например HDR-filter должен стоять до tone map, LDR-filter — после него.
4. Соблюдите descriptor/push constant/vertex ABI и отразите любые новые ресурсы в manifest.
5. Примените HLSL, проверьте эффект на кадре и ошибку компиляции с сохранением последней рабочей версии.
6. Зарегистрируйте внешние файлы/include, экспортируйте и пересоберите пакет.

Для постоянного встроенного прохода добавьте исходник в assets/gpu, descriptor и builtin_manifest/source в module-engine-vulkan, затем исполнение/валидацию планировщика. Изменение контракта без нового ID требует явной миграции графов.

## Новый вид визуального Rust

Добавляйте поддержку в module-engine-custom только вместе с точным соответствием диапазонам исходника, проверкой типов/допустимых правок и сохранением остального текста. Для непонятного фрагмента нужен Opaque, а не выдуманное исполнение. Командный граф и scalar-граф имеют разные ограничения; возможность обычного rustc compile не означает визуальную поддержку всех конструкций.

Полезные проверки расширения: эффект на реальные snapshots/пиксели, отказ неверного контракта до изменения состояния, сохранение текста/позиции и экспорт без зависимости от оригинального абсолютного пути. Общие тесты не заменяют такую проверку.

## Вклад typed настроек модуля

ModuleDescriptor.properties содержит PropertySchema. Реальный legacy пример motion_property_module в core/settings.rs предоставляет game.motion.speed: Number 0..=8, default 1, scope Component {component:"game.motion"}. Его читает compiled controller прежнего project.json. В новой авторской игре компоненты хранятся отдельно в game.scenes/templates, а активное участие объявляют descriptors игровых систем. Оба примера исполняют данные; это разные границы хранения/редактирования.

Для своего вклада:

1. Задайте стабильные namespaced module/field/component ID, PropertyType, default и scope. Number bounds включительны; Text ограничен UTF-8 bytes.
2. Объявите requires_modules/components. Не связывайте видимость с одним присутствием descriptor в palette: field нужен actual active module и participant.
3. Зарегистрируйте descriptor через register_module; field-only feature может иметь пустые inputs/outputs. Для runtime поведения предоставьте настоящую реализацию, descriptor сам её не создаёт.
4. Добавьте feature в engine.enabled_modules либо подключите исполняемый модуль. attach_component(document,registry,object_id,component_id) добавляет участие; reconcile_properties переводит отключённые значения в archive и восстанавливает совместимые.
5. Читайте ctx.parameter, ctx.property, ctx.component_property либо ctx.properties.global/modules. Runtime выдаёт resolved active values/defaults; inactive не передаётся.
6. Для Project renderer control автор Engine задаёт ExposedParameter target/range и VariantGroup. Проверяйте команды compiled Rust тем же policy, а не только ограничивайте widget.
7. Проверьте actual gameplay/GPU effect, отсутствие поля у nonparticipant, detach/restore, bad value refusal и Team export без inactive data. Пересоберите native sources и обновите source manifest/implementation pin сознательно.

Динамический UI Number/Boolean/Enum/Text общий и не требует отдельной панели для каждого field. Networking descriptor в тестах проверяет participation/dependencies; он не является сетевой реализацией.


