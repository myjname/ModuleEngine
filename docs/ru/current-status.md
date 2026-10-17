# Текущее состояние ModuleEngine

[English](../current-status.md) | [Русский](current-status.md) | [Документация](README.md)

Сводка на 17 октября 2026 года, исходная база прежнего документационного этапа — `c64bc34ffd28419cfd851838c16a2a48c9c2fcf8`. Это указатель актуальных контрактов и накопленных доказательств, а не новая приёмка HEAD или готового EXE. Количества тестов и измерения относятся только к snapshot каждого отчёта.

## Цели и границы

Общие модули и типизированные контракты должны работать независимо от жанра и конкретного примера. Авторские данные отделены от Play; отказ изменения сохраняет рабочую версию и пользовательскую работу. Engine владеет архитектурой, Project меняет разрешённое. Player — отдельная выбранная native-поставка без Editor, Rust/Cargo/DXC на машине игрока.

[Решения](decisions.md) сохраняют принятый объём и допущения, [открытые вопросы](questions.md) — будущие варианты. Исторические фразы «остаётся следующим этапом» в старых отчётах не переоткрывают уже завершённую работу. Скелетная анимация, полный PBR, navmesh, сетевой режим и другие большие подсистемы требуют отдельной задачи и критериев.

## Реализовано

| Область | Актуальный контракт | Доказательства и описание |
| --- | --- | --- |
| Общий Host и ECS | Shared GameHost вне Editor; Core нейтрален к ARPG, изменяемый мир принадлежит ECS. | [Завершённые этапы 2–8](modular-runtime-progress.md), [Host](shared-game-host.md), [ECS](ecs-world.md). |
| Игровые пакеты и Player | ARPG опционален; Two Arenas и независимый Puzzle — отдельные проекты. Player имеет отдельный executable и явно выбранные native features. | [Границы пакетов](optional-gameplay.md), [Player crate](../../crates/player/README.ru.md), этапы 5–7 в [журнале](modular-runtime-progress.md). |
| Custom после Stop | При неизменённом supported compile context готовый код переиспользуется; freshness proof может вызывать Cargo без новой root compilation. State и worker PID экземпляров независимы. | [Compiled-program store](custom-program-store.md), [последующая Windows/Linux проверка](remaining-authoring-fixes-20261016.md). |
| Defaults и Inspector | PropertySchema asset defaults учитываются с фактическими live IDs; точные JSON-ключи сохраняются; Apply без изменений сохраняет prefab inheritance. | [Исправления и регрессии](remaining-authoring-fixes-20261016.md), code snapshot `fb63b3ddae7cad52532761932855b755c61fb2c7`. |
| Windows dialogs и Save As | Начальная папка — проверенная необязательная подсказка. Save As публикует кандидата до принятия в Editor; Cancel/error сохраняют исходную работу. Recovery-файл при отказе записи текущего файла допустим. | [Проверка диалогов и транзакции](dialog-path-fixes-20261016.md), code snapshot `0fbfb9ca74a139253b4e9d611b2b8877cfb4166d`. |
| Linux | Native Editor, Player и Full/Team exports проверены в существующей Ubuntu/WSL2; окна X11/Wayland — через WSLg/llvmpipe. Это ограниченная функциональная поддержка. | [Условия запуска](linux-support.md), [authoring acceptance](remaining-authoring-fixes-20261016.md), [последний dialog-этап](dialog-path-fixes-20261016.md). |

## Открыто и не подтверждено

- **Строгая цель Player 60 FPS, 1080p, 20 активных врагов не достигнута** в последних относящихся к ней замерах. Callback FPS и функциональное завершение не заменяют mean/p95 cadence уникальных игровых кадров. См. [финальные результаты](review-fixes-20261016.md).
- **Холодная подготовка Editor с Custom в пределах прежних 45 секунд остаётся открытой.** Успешный prewarmed Editor и маленький Puzzle-сценарий её не подтверждают. См. [границы authoring-этапа](remaining-authoring-fixes-20261016.md) и [объём dialog smoke](dialog-path-fixes-20261016.md).
- **Native Linux WAV и File/Folder dialogs отсутствуют.** Проверочный проект без audio и CLI-операции не подтверждают эти сервисы; production-данные нельзя молча лишать звука. См. [Linux services](linux-support.md#опциональные-сервисы).
- **Аппаратный Linux, переносимость на произвольные ABI/драйверы и ручная foreground input-to-display QA не сертифицированы.** WSLg software Vulkan и offscreen package verify имеют другой объём проверки.
- Custom worker управляет временем жизни процессов, но **не является sandbox для недоверенного Rust**. Синхронный GPU readback, ограниченное визуальное подмножество Rust и прочие функциональные границы остаются в тематических документах.

## Как выбирать доказательства

Последний code snapshot исправлений диалогов — `0fbfb9ca74a139253b4e9d611b2b8877cfb4166d`: [отчёт](dialog-path-fixes-20261016.md) относится к трём изменённым Rust-файлам, Editor tests и перечисленным native сценариям. Полный workspace Windows/Linux из [предыдущего authoring-этапа](remaining-authoring-fixes-20261016.md) относится к `fb63b3ddae7cad52532761932855b755c61fb2c7`; его числа нельзя объявлять новым полным прогоном текущих исходников. Performance-статус из [review-fixes](review-fixes-20261016.md) не изменился от последующих успешных функциональных проверок.

Готовые `outputs/ModuleEngine.exe`, `ModuleEngine-review-fixed.exe`, `ModuleEngine-authoring-fixed.exe` и `ModuleEngine-dialogs-fixed.exe` относятся к разным source snapshots. Выбирайте артефакт по его отчёту и SHA; дата этой страницы не обновляет EXE или source inventory.

## Маршруты проверок

- Windows: корневой [verify.ps1](../../verify.ps1); необязательная ветка `-NativeBenchmarks` требует рабочего стола и изолированного замера.
- Linux: корневой [verify.sh](../../verify.sh), условия native toolchain/DXC/Vulkan — в [Linux guide](linux-support.md). Скрипт включает offscreen проверки и не закрывает desktop/export acceptance.
- [Реестр и stdlib launcher](verification.md) добавляют явные `quick`, `contracts`, `clippy`, `native`, `native-tools`, `package`; `--plan`/`--list` читают конфигурацию без Cargo. Native tests запускаются напрямую из CargoJSON-selected executable с normal loader environment. Windows-only проверки на Linux отмечаются `skipped`. `package` проверяет минимальные Cargo product/snapshot fixtures, а не полный Full/Team/Player export.
- Объём legacy-вызовов без аргументов сохранён; Windows `-NativeBenchmarks` выбирает actual normal Editor из CargoJSON и по-прежнему не требует Python. Новый launcher требует уже установленный Python 3.9+ без сторонних зависимостей. Performance остаётся отдельным opt-in legacy/адресным маршрутом с прежними ограничениями.

Launcher и его unit fixtures реализованы отдельно от Rust. Unit-проверки и планирование не являются новой приёмкой engine/native/package; реальные профили требуют своих отчётов на текущем snapshot. Правила работы и сохранности данных: [AGENTS.md](agent-guide.md).

Три [проверки свойств Inspector](verification.md#проверки-свойств-inspector) добавляют 99 конечных сценариев через реальный Apply/save/reopen: точные ключи и null, opaque local payload, будущее наследование prefab/defaults и намеренные локальные правки. Это тестовое усиление существующего поведения; новые зависимости и продуктовые изменения не требовались. Проверка чувствительности действительно обнаружила временно возвращённую ошибку разбора `literal.key` как dot path; правильная реализация затем восстановлена побайтно.

На этом этапе `contracts` прошёл на Windows и Ubuntu/WSL (по 178 tests; platform-specific наборы отличаются); Windows `native`/`package` проверены отдельно на этапе маршрута. [Результаты и границы внедрения](verification.md#проверка-внедрения-16-октября-2026) не заменяют полную приёмку движка или опубликованных EXE.

17 октября в `contracts` добавлены четыре существующих Save As tests и три
[проверки сохранности workflow](verification.md#сохранность-save-as-и-файловой-истории)
с 28 конечными сценариями. Windows и Ubuntu/WSL прошли по 185 Rust tests на исходнике
`editor_workflow.rs` SHA-256 `88e8f1d12cc01ce369d5e016e2107fdeb42b5706ab93078047b56b4a8a4aa1cf`.
Временное отключение rollback вызвало реальный отказ по изменившимся файлам;
исходник восстановлен побайтно, адресный тест затем прошёл. Это тестовое усиление,
без изменения production-поведения или Cargo dependencies; прежние performance
и delivery ограничения сохраняются.

[GitHub CI](ci.md) запускает отдельные Windows/Linux проверки `contracts` и `clippy`,
включая документационные PR, и выбранные native/package
маршруты; в публичном `myjname/ModuleEngine` для `main` требуется `CI required`,
включая администратора. Новые версии оцениваются по фактическому CI своего SHA,
не по старому зелёному запуску. Негативный служебный PR подтвердил блокировку merge.

17 октября добавлен явный профиль `clippy` для всего workspace, всех targets и features
с запретом warnings и проверкой pinned Rust release/native target.
Он не добавляет formatting, tests или runtime-проверки. Профиль прошёл на
Windows (Clippy 3.359 с; preflight 0.219 с) и Ubuntu/WSL (Clippy 27.856 с;
preflight 0.592 с) с существующими прогретыми общими caches. Отчёты:
[Windows](../../outputs/ci-editor-builds-20261017/verification/20261017T015811Z-985d823c/result.json),
[Ubuntu/WSL](../../outputs/ci-editor-builds-20261017/verification/20261017T015921Z-6ef11385/result.json).
Это не холодные замеры и не аппаратная приёмка.
[Hosted-прогон этапов 1/2](https://github.com/myjname/ModuleEngine/actions/runs/38994338494)
прошёл Windows/Linux Clippy, contracts, выбранные native/package маршруты и aggregate;
прежние границы приёмки сохраняются.

[Маршрут диагностики Editor](editor-diagnostics.md) использует предоставленный обычный
executable, сгенерированные fixtures и изолированные Recent/layout.
[Windows-прогон both](../../outputs/ci-editor-builds-20261017/diagnostics/20261017T020938Z-e9e189aad466/result.json)
прошёл workflow и native WGPU/Dx12 capture; проверки неподдерживаемой оконной системы
и timeout отклонили свои случаи с доказательствами сохранности и owned-process cleanup.
Финальный Windows `contracts` прошёл 187 Rust tests в 16 шагах, Clippy прошёл,
полный Python suite прошёл с 66 тестами и двумя явными platform skips; ссылки на
отчёты приведены в руководстве. Native панель Scene в both-прогоне ещё готовилась;
отдельные workflow PNG проверяют импортированную модель, а Puzzle capture показывает поле.
Linux native capture ещё не подтверждён; hosted-прогон этапов 1/2 прошёл
перечисленные CI-маршруты. Это не доказательство готовности Scene, cold-start,
отзывчивости compositor или полной поставки.

[Замеры сборки/проверок и cache policy](build-verification-performance.md) разделяют
target-cold компиляцию и повторные тесты: оба локальных цикла `contracts` прошли
187 тестов без изменения исходников/защищённых данных. Полное время цикла —
182.375/89.906 с; это не доказывает hosted выигрыш или cold-start Editor.
Консервативный CI Cargo cache сохраняет каждый шаг проверки; реальные hosted
baseline/miss/hit результаты относятся к своим отчётам с учётом переноса/save.

[Независимое ревью](review.md) обязательно для существенных изменений по AGENTS.
Другой агент/человек проверяет diff, цель, реальные пути и evidence. Stdlib
`review_snapshot.py` связывает заключение с HEAD и байтами tracked/untracked source;
его успешная сверка сама не является approval. [Шаблон](review-template.md)
сохраняет замечания, исправления, повторную проверку и непроверенные границы.

## Рабочий процесс ассистента

17 октября добавлены [правила общения и контекста](assistant-workflow.md),
репозиторный skill для дебага и адаптивные настройки подзадач. Значения по умолчанию
для новых подзадач — `gpt-6.1-sol` / `medium`; явные запуски выбирают поддерживаемое
усилие по задаче. Это не меняет активный родительский turn или глобальный `ultra`
пользователя. [Пилот Serena в режиме чтения](assistant-efficiency-20261017.md)
нашёл девять проверенных ссылок вызывающего кода после исправления загрузки workspace.
Общее ускорение и снижение расходов аккаунта не измерены; постоянная MCP-регистрация
не добавлена. Эти изменения относятся к инструментам ассистента и документации,
а не к приёмке движка/native.
