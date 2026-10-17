# Диагностика редактора

[English](../editor-diagnostics.md) | [Русский](editor-diagnostics.md) | [Документация](README.md)

[scripts/editor_diagnostics.py](../../scripts/editor_diagnostics.py) выполняет ограниченную
диагностику с существующим обычным native Editor executable. Он создаёт собственный
fixture, использует прежний `--editor-ui-check` для workflow и записывает native
WGPU captures. Wrapper не собирает Editor и не принимает авторский JSON проекта.
Приведённые ниже Windows-прогоны подтверждают этот ограниченный маршрут; Linux
native capture и hosted CI этого изменения ещё не проверены.

## Запуск

Используйте установленный Python, поддерживающий launcher, и абсолютный путь к
существующему обычному Editor executable текущих ОС/архитектуры. Приложите его build
report, чтобы сопоставить SHA executable с нужным source snapshot. Test harness
или старый executable без diagnostic protocol не подходит. Linux требует обычного
loader/toolchain environment и работающего X11 или Wayland display; см.
[требования Linux](linux-support.md). Workflow export может использовать существующие
build tools и caches; сам wrapper не устанавливает инструменты и не восстанавливает
зависимости.

```powershell
& $python scripts/editor_diagnostics.py --editor 'E:\ModuleEngine\outputs\YourVerifiedEditor.exe' --scenario capture --view scene --menu File
& $python scripts/editor_diagnostics.py --editor 'E:\ModuleEngine\outputs\YourVerifiedEditor.exe' --scenario both --timeout 1800
```

Замените пример пути фактическим проверенным executable. Пример Linux:

```sh
python3 scripts/editor_diagnostics.py --editor /absolute/path/to/module-engine-editor --scenario capture --window-system x11
```

| Параметр | Контракт |
| --- | --- |
| `--editor` | Обязательный абсолютный путь к существующему native PE/ELF executable текущей архитектуры. |
| `--scenario capture\|workflow\|both` | По умолчанию `capture`. Capture создаёт новый Puzzle через `--new-puzzle`; workflow/both создают новый Two Arenas fixture через `--new-game`. `both` выполняет workflow до capture. |
| `--view` | Capture view: `render`, `shader`, `history`, `engine`, `settings`, `game`, `scene`, `assets`, `systems`, `custom`; по умолчанию `scene`. |
| `--menu` | Необязательное меню capture: `File`, `Edit`, `Tools`, `Windows`. |
| `--window-system auto\|x11\|wayland` | По умолчанию `auto`. Явный выбор должен совпасть с инициализированной оконной системой. Windows capture обязан инициализировать оконную систему Windows. |
| `--timeout` | Положительное конечное число секунд на каждую команду, по умолчанию 180. Для workflow builds явно укажите `--timeout 1800`; product watchdogs не меняются. |
| `--output-root` | Необязательный корень отчётов внутри `outputs/` этого checkout; по умолчанию `outputs/editor-diagnostics`. Каждый вызов создаёт новый дочерний каталог timestamp/nonce, сохраняя прежние доказательства. |

В примере `$python` должен указывать на настоящий Python interpreter. Команды
не выбирают сборку автоматически. Сохраните выбранный путь и build evidence вместе
с прогоном.

## Изоляция и сохранность

Каждый запуск размещает сгенерированный fixture и изолированный personal directory
в `work/verification/editor-diagnostics/<run-id>/`. Дочерние TEMP/TMP/TMPDIR используют
уникальный короткий `.work/v/<token>`. Wrapper удаляет унаследованные capture controls
и явно выбирает WGPU и запрошенную оконную систему, сохраняя обычное loader
окружение. Запрошенный размер окна 1580×940, минимальный 1100×720; фактические
размеры PNG проверяются и записываются.

Явная opt-in переменная `MODULE_ENGINE_DIAGNOSTIC_USER_ROOT` должна указывать на
существующий абсолютный изолированный каталог. Wrapper создаёт его и передаёт
дочерним процессам. Перенаправляется только сохранение Recent и layout. Это не
отменяет проверку package/source, SDK pins, Engine binding или Full/Team permissions.
Без переменной обычное поведение Editor не меняется; неверный явно заданный root
даёт отказ без fallback к установленным personal files.

Маршрут использует общий с verification launcher `work/verification/launcher.lock`;
конкурирующий launcher не получает lease. Это не сериализует ручной Cargo или другие
программы. Не выполняйте параллельные builds/native операции и правки исходников
во время запуска. Защищённые авторские пути, включая personal Recent/layout,
инвентаризируются до и после исполнения, в том числе после failure/timeout.
Изменения означают отказ; автоматического restore или очистки файлов пользователя
нет. SHA предоставленного executable и сгенерированного fixture должны сохраняться
весь прогон.

## Доказательства и отказ

`result.json` записывает команды, пути stdout/stderr, exit status, время каждой
команды, timeout/owned-process cleanup evidence, выбранное окружение, путь/SHA
executable, путь/SHA сгенерированного fixture, inventory защищённых файлов и
результаты сохранности. Также записываются HEAD checkout и digest inventory
tracked/untracked исходников до и после запуска. Inventory описывает checkout,
но не доказывает, что предоставленный executable собран из него. Перед выводом
о конкретных исходниках сопоставьте executable с отдельным build evidence.

Одного exit 0 Editor недостаточно. Capture требует новый полный RGBA8 PNG с
ограниченными положительными размерами, корректными chunks/CRC, завершённым zlib
и полными scanlines. Также требуется ровно одна запись фактически инициализированной
графики: WGPU с `Dx12` на Windows или `Vulkan` на Linux и совместимая фактическая
оконная система. Одни запрошенные значения окружения не доказывают графику.
`native-ui.png` — application WGPU readback, а не compositor screenshot.

Workflow требует новый `workflow-report.json`, `passed=true`, положительные целые
`model_pixels_changed` и `model_primitives`, все четыре `menus_clicked` равными true.
Также необходимы true `new_templates_preserve_engine`,
`copy_retains_author_document`, `unassigned_asset_copied`,
`windows_reuses_current_workspace`, `exported_player_pixels_equal` и
`original_project_unchanged`, плюс проверенные `relocated-model.png` и
`player-model.png`. Проверяются существующие сервисы, операции меню egui и
offscreen рендеринг model/Player, а не взаимодействие с native file picker.

Итог различает `passed`, `failed` и `timed_out`. Отказ команды, отсутствующие или
неверные доказательства либо нарушение сохранности отклоняют прогон. Timeout cleanup
завершает только запущенное дерево/group процессов через общий launcher executor;
результат записывается. После создания каталога отчёта failures сохраняют logs и
`result.json`. Неверные начальные аргументы/пути executable могут дать отказ до
создания отчёта. Exit codes: 0 для passed report, 1 для failed/timed-out report,
2 для начального отказа setup/argument validation.

## Доказательства Windows от 17 октября 2026

[Build report](../../outputs/ci-editor-builds-20261017/stage2-editor-build/result.json)
указывает обычный Editor, выбранный из CargoJSON, с SHA-256 executable
`5ccefd946b718ca523f732ff54d8c6cbe793ecb6afc06f19208dd1877da8c388`.
[Сверка inputs](../../outputs/ci-editor-builds-20261017/stage2-executable-input-check.json)
подтверждает совпадение всех 143 Rust/Cargo source inputs с этой сборкой. Первый
[Puzzle capture](../../outputs/ci-editor-builds-20261017/diagnostics/20261017T020736Z-ae5f9d5f527c/result.json)
прошёл с открытым File и отрисованным полем Puzzle при 1580×940 на WGPU/Dx12, NVIDIA RTX 4080 SUPER.

Сначала workflow завершился внутри Editor, но Python wrapper ошибочно отклонил его
из-за обработки Windows canonical paths с `\\?\`. Минимальный probe подтвердил
один и тот же parent directory. Теперь containment check использует fallback через
сравнение same-file ancestors; regression отказал до исправления и прошёл после,
при этом внешние пути по-прежнему отклоняются. Новый полный
[прогон both](../../outputs/ci-editor-builds-20261017/diagnostics/20261017T020938Z-e9e189aad466/result.json)
прошёл три команды и три captures, включая menus, Save As, import, export,
30 719 изменённых model pixels в отдельных workflow model PNG и native capture
панели Scene во время подготовки Scene/Rust Custom. Корректный screenshot и
инициализированный backend не доказывают завершение подготовки или отрисовку этой Scene.

Негативные прогоны отклонили
[неподдерживаемый запрос Wayland на Windows](../../outputs/ci-editor-builds-20261017/diagnostics/20261017T020954Z-1c2e019218a1/result.json)
и записали явные timeout для
[создания fixture с пределом 0.01 с](../../outputs/ci-editor-builds-20261017/diagnostics/20261017T020955Z-f624acdcac14/result.json)
(одна команда) и
[native capture с пределом 0.2 с](../../outputs/ci-editor-builds-20261017/diagnostics/20261017T021008Z-e53a1235b6ac/result.json)
(две команды), с записью owned-process cleanup. У native-capture timeout
`cleanup_exit_code=0`. При fixture timeout 0.01 с возникла гонка с нормальным выходом
child; cleanup записал exit 128/error, а прогон правильно остался `timed_out`.
Каждый diagnostic report по ссылкам
содержит `protected_unchanged=true` и `source_unchanged=true`; предоставленный
executable и созданный fixture сохранили SHA. Сравнение исходников относится к
каждому прогону отдельно, а не к последующим правкам документации.

Прошли два реальных Rust regression tests и 11 Python launcher tests, включая
Windows canonical-path regression. Финальный Windows
[`contracts`](../../outputs/ci-editor-builds-20261017/verification/20261017T021011Z-9d643200/result.json)
прошёл 187 Rust tests в 16 шагах; также прошёл
[`clippy`](../../outputs/ci-editor-builds-20261017/verification/20261017T021244Z-aad65aea/result.json).
Финальный полный
[Python suite](../../outputs/ci-editor-builds-20261017/stage2-final-python-tests.json)
прошёл с 66 тестами и двумя явными platform skips; ссылки на logs находятся в
отчёте. Linux native capture и hosted CI остаются непроверенными. Ограничения объёма
ниже сохраняются.

## Объём и проверка

Два Rust-теста `diagnostic_personal` в `contracts` проверяют validation явного root
и изоляцию Recent/layout с сохранением прочего Editor state и source permissions.
Native capture они не запускают. Unit fixtures wrapper проверяют protocol validation
и обработку отказов; они не заменяют реальные запуски с предоставленным Editor
Windows/Linux. Исторические числа contracts относятся к исходным snapshots;
добавление этих тестов не обновляет старые отчёты.

Маршрут не подтверждает отзывчивость DWM/compositor, foreground input latency,
время cold-start, native picker, hardware Linux, pixel-perfect вид UI или полную
приёмку Full/Team/Player. Оценивайте captures и отчёты в границах конкретных сценария
и артефакта. См. [маршруты проверок](verification.md) и
[работу с проектом в Editor](editor-workflow.md).
