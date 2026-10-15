# Настройки Engine и Project

EngineConfiguration включает feature-модули и задаёт разрешённые Project controls. ProjectConfiguration хранит значения, ready variants и условия. PropertyStore хранит module-contributed данные отдельно от графа архитектуры.

В Project Settings измените exposure и выберите ACES/Reinhard либо FXAA/off. Это реальные GPU варианты. Условия по времени/параметру выбирают готовый вариант; первое совпавшее правило группы имеет приоритет, иначе используется selection/default. Отсутствующий вариант даёт ошибку без молчаливой замены.

В прежнем демонстрационном project.json выберите объект 1 в Object Tree. Legacy Object Inspector показывает game.motion.speed компонента game.motion. Сохранённое значение читает его compiled controller; это отдельный пример typed PropertyStore, не игровой контроллер нового game-project.json:

~~~rust
if let Some(speed) = ctx.component_property(1, "game.motion", "game.motion.speed")
    .and_then(|value| value.as_f64())
{
    ctx.set_position(1, [(ctx.time * speed).sin() as f32, 0.4, 0.0]);
}
~~~

Другие объекты не получают поле из-за одного установленного descriptor. Attach добавляет участие; Detach архивирует значения. Отключение feature-модуля убирает вклад из активных views/runtime, сохраняя данные для восстановления. Archive входит в Full save/export и local history, исключается из новой Team поставки.

В legacy PropertyStore persistent values привязаны к начальным объектам demo, а Transform/Color старого Object Inspector меняют live preview. Для сохраняемых игровых сцен используйте Game / Scene: Transform/Color, components, references, prefab и overrides записываются в game.scenes. Новые объекты создаются в этом авторском окне; runtime spawn во время Play не переносится в источник автоматически. Настройки actor/health/melee нового примера принадлежат этим компонентам, без legacy Scalar-цепочки.

PropertySchema содержит namespaced id, label, scope, value_type, default, requires_modules/components. Scope: Global, Module, Object, Component {component}. Типы: конечный Number с включительным min/max, Boolean, Enum и Text с лимитом UTF-8 bytes. Видимость зависит от actual active modules/component participation.

Global/Module поля редактируются в Project Settings; Object/Component фильтруются по выбранному объекту. resolved_properties подставляет defaults; archive не передаётся в ScriptContext. ctx.parameter/property/component_property и ctx.properties.global/modules читают активные resolved values.

Engine Exposure связывает exposed numeric scalar с pass ID/name/range. VariantGroup содержит ready варианты разрешённых RenderCommand. Состав/порядок вариантов задаёт Engine; Project выбирает варианты/условия. Текущая renderer parameter граница числовая; Boolean/Enum choices pipeline представлены ready variants.

Compiled команды проверяются той же policy: необъявленный target, out-of-range и запрещённый pass отклоняются до изменения состояния. Legacy schema1/2 читается; GUI/Team инициализирует новую policy. Team требует initialized:true.

Networking schema есть только в тестах. Этот механизм позволяет модулю внести собственные поля; он не означает готовой сетевой подсистемы.

## Компоненты игрового маршрута 0.4

SystemDescriptor.components объявляет вклад подключённой игровой системы. Нормальный инспектор фильтрует по активному модулю и наличию компонента у объекта; отдельный Advanced JSON показывает сохранённый авторский архив. Игра получает только активные компоненты, а Team/Player экспорт подготавливает такую же копию. Структура полей текущей формы берётся из JSON-значений: полноценные enum/range/help schemas игровых компонентов ещё не предоставлены всем native-модулям.

Шаблон хранит defaults; индивидуальная правка наследованного поля записывается отдельным dot-path override. Правка defaults действует на остальные экземпляры. Порядок систем, bindings, after и список подключённых модулей принадлежат Engine и заморожены для Team. Подробности и кнопки — [editor-game.md](editor-game.md), формат данных — [scenes.md](scenes.md).


