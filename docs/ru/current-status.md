# Текущее состояние ModuleEngine

[English](../current-status.md) | [Русский](current-status.md) | [Документация](README.md)

Сводка на 18 октября 2026 года: [текущее завершение P0](p0-finalization-20261018.md) фиксирует полные применимые проверки Windows/Linux и конкретные наблюдения реального интерфейса. Приёмка по исходникам определяется [внешним итоговым ревью](../../outputs/reviews/p0-finalization-20261018/final-review.md). [P0 17 октября](p0-stabilization-20261017.md), [продолжение до async](p0-continuation-20261018.md) и [async-доказательства](gpu-async-20261018.md) сохраняют собственные артефакты и ограничения; дата страницы не становится новой приёмкой EXE.

## Цели и границы

При подготовке PR также устранён пробел проверки полноты исходников Full: `crates/render-vulkan/src/retirement.rs` теперь обязателен в inventory исходников. Его отсутствие запрещает авторское редактирование Engine; регрессия исключения исходника проверяет этот дополнительный контракт упаковки отдельно от исторических замеров асинхронного runtime.

Тесты workflow и персональных раскладок теперь используют [изолированные установки исходников с обычной проверкой](verification.md#isolated-source-fixtures), поэтому устаревший manifest checkout не останавливает их CI-сценарии до целевых assertions. Изменена подготовка тестов; приёмка runtime по-прежнему привязана к отчётам ниже.

Общие модули и типизированные контракты должны работать независимо от жанра и конкретного примера. Авторские данные отделены от Play; отказ изменения сохраняет рабочую версию и пользовательскую работу. Engine владеет архитектурой, Project меняет разрешённое. Player — отдельная выбранная native-поставка без Editor, Rust/Cargo/DXC на машине игрока.

Принятая долгосрочная цель — [портфель 5–10 разных законченных игр](portfolio-roadmap.md),
охватывающих 2D/2.5D/пространственное 3D, пиксельную/полигональную/реалистичную/
мобильную графику и разные механики. Мобильная стилистика сейчас означает игру на ПК;
мобильные порты относятся к будущему, конкретные платформы ещё не выбраны.
План предлагает покрытие, этапы возможностей
и критерии приёмки; дизайны игр отдельно не утверждены, запись плана не принимает
ни одного участника. Two Arenas и Puzzle остаются техническими основами до выполнения
этих критериев. Это новое направление продукта, а не новые runtime-возможности.

Теперь P0 требует аудита и стабилизации существующей версии до P1:
исправить все подтверждённые дефекты текущего объёма и измеренные проблемы
производительности, повторить обязательные проверки и выполнить неизменённые
критерии ARPG и холодной подготовки Custom. Отсутствующие будущие подсистемы
остаются отдельной работой. Это принятое изменение плана; запись сама не закрывает критерии доказательств.

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

- **Native Linux WAV и File/Folder dialogs отсутствуют.** Проверочный проект без audio и CLI-операции не подтверждают эти сервисы; production-данные нельзя молча лишать звука. См. [Linux services](linux-support.md#опциональные-сервисы).
- **Аппаратный Linux, переносимость на произвольные ABI/драйверы и ручная foreground input-to-display QA не сертифицированы.** WSLg software Vulkan и offscreen package verify имеют другой объём проверки.
- Custom worker управляет временем жизни процессов, но **не является sandbox для недоверенного Rust**. Явный диагностический GPU readback, ограниченное фоновое ожидание освобождения GPU, ограниченное визуальное подмножество Rust и прочие функциональные границы остаются в тематических документах.

<a id="p0-finalization-20261018"></a>

### Текущее завершение P0 — доказательства и решение

Фактический обычный Editor candidate09 `a3106737…` отличается от измеренного source08 `dcaaf0f3…` при равных 150/163 native-входах. Его шесть неизменённых CLI-маршрутов, новая cold и реальная пара ARPG Editor/перенесённого Player: PASS: [CLI 6/6](../../outputs/verification/p0-finalization-20261018/windows-cli-final-09/result.json), [новая cold](../../outputs/verification/p0-finalization-20261018/cold-final-09/launch.json), [текущая ARPG пара](../../outputs/verification/p0-finalization-20261018/arpg-final-09/result.json); прежние временные/физические результаты source08 не переименовываются в новые измерения артефакта. Итоговая привязка следует source10/normal10/новому Full10/data-proof10 в отчёте и внешнему решению.

Source08 проходит полные обычные прогоны workspace: **Windows 712 обычных/9 ignored; Linux 710/8**, каждый со всеми 14 фактическими артефактами и явно записанным Player-bin harness без тестов. Обязательные native/package тесты проходят отдельно: Windows 7; Linux 6 плюс явный Windows-only executable-spelling skip. Fixture-emitter и историческая catchup-диагностика остаются исключёнными. Строгие статические/конфигурационные проверки, реальные поставки Full/Team/выбранного Player и текущие GPU Windows 27/16/Linux 28/16 проходят; количества и привязки исходников/артефактов приведены в [отчёте](p0-finalization-20261018.md).

Повторно проверенные native ARPG окно обычного Windows Editor candidate09 `a3106737…` и новый перенесённый Player `4786c95a…` проходят неизменённый критерий 1080p/20 врагов в Playing: настоящий UI FPS **119.0092/119.8802**, p95 **14.1886/14.0567 мс**, нулевые измеренные CPU pixels/readback/upload. Новый target-cold проходит за **36.172 с wall / 35.1084651 с internal**, включая итоговую готовность после Stop. Прежние метрики source08 на `dcaaf0f3…` сохранены отдельно в отчёте. Эти проверки не доказывают причинную A/B экономию или гарантии чистой машины/scanout. Linux Puzzle с программным Vulkan остаётся функциональной проверкой с `target_met=false`, не успехом временного критерия.

Foreground01 фиксирует системные Unicode Save As, изменения Scene/save/reopen, Cancel/Undo, отклонённую связь графа, Import Cancel и Play/restart/Stop с самостоятельным выходом и сохранностью. Точные 15 изменений авторской копии и шесть групп без новой физической сертификации указаны явно; широкий checklist помощника не создаёт новых критериев. Прежние отказы остаются отказами. Итоговые записи документации/поставки инструментов и [независимое итоговое решение](../../outputs/reviews/p0-finalization-20261018/final-review.md) определяют завершение текущей P0; ограничения аппаратного Linux, native Linux audio/диалогов и физического input-to-display сохраняются.

### Асинхронный производитель GPU: ограниченные runtime-критерии PASS — 18 октября 2026

Обычный нативный Vulkan теперь отправляет один pending-запрос и опрашивает точный ticket без обычного ожидания GPU производителя или CPU-передачи пикселей. Четыре аренды и фактическое завершение потребителя ограничивают повторное использование; отправленные камера/revision/tick/происхождение и владельцы Scene остаются связаны со своими пикселями. Явная диагностика сохраняет синхронное чтение; лишь владеющее фоновое освобождение может ждать собственный fence до 30 с, удерживая неподтверждённые ресурсы/устройство при ошибке. Fixed 60 Hz-симуляция и цель показа 120 Hz неизменны.

[Асинхронный GPU-отчёт](gpu-async-20261018.md) привязывает Windows08 `318ae8c06c406c959aad706ecf16efbe45e368b648d62f2ebf8176ba5316d8fe`: текущие 27/16 GPU-сценариев/точных изображений,307 обычных/3 ignored scoped-тестов и два отдельно выполненных ignored native UI-теста, строгий Clippy, тёплый полный Editor Graph/Scene/Game+Play/Stop, выбранный 18-файловый tools-free Player/перемещение и AB03 все 12 процессов. Editor A/B означает нативное ARPG-окно обычного артефакта Editor; standalone Player проверяется отдельно. Принятые дополнительные аудиты восстанавливают фактические UI/callback/worker-серии, исправляя подписи helper F4 без изменения исходных отчётов. Медиана worker FPS Editor 120,0422→119,0027/Player 119,0875→120,0249; worker p95 растёт на 0,3543/0,3731 мс. Фактический UI p9514,0170→14,0375/14,0813→13,9872 мс. Неизменённые ограниченные критерии проходят; общее отсутствие ухудшения или универсальное улучшение не заявляются.

Linux L01 `73a77fe8e06bd1a10fcc99f876c4dc7b7e612b396ab00c6c23c9942cfc142066` отдельно привязывает реальные 28/16 offscreen, обычный Editor, строгий Clippy, X11 resize1280×800→1440×900/Graph/Scene/Game/PlayStop/естественный выход и обычноеWM_DELETEзакрытие empty-fixture с исчезновением всех принадлежащих операции групп без засчитывания принудительной очистки успехом. Scoped-покрытие — непересекающееся логическое объединение 303 обычных/3 ignored между успешными Core/Host неудачного v1 и пятью оставшимися suites v2, не один full-workspace-запуск. JSON-only proof устанавливает равенство 150 native/163 complete-входов Windows08; WSL дополнительно учитывает лишь непрозрачную настройку. Исходные pre-Cargo guard-ошибка и ошибка executable-bit fixture на DrvFS остаются неудачами с явными квитанциями. WSLg/llvmpipe — ограниченные функциональные доказательства; native Linux audio/dialog, hardware/ABI/реальная потеря устройства, физический scanout и новая cold-сертификация вне их.

Отчёт ссылается на внешний итоговый inventory исходников и независимое заключение; интерпретация ограничена записанным в них объёмом. Runtime-хеши остаются Windows Editor `cebd1391…` / Linux ELF `e7ad6a75…`; планируемый исходный Full и отдельно сохранённый обычный EXE имеют собственную последующую квитанцию поставки. Новая приёмка всегоP0/workspace не заявлена. Исторический P0 от17 октября ниже не меняется.

### Исторический GPU-этап до async — 18 октября 2026

[Квитанция ME-01](../../outputs/verification/p0-continuation-20261018/me01-receipt.json) сопоставляет унаследованный снимок исходников с конкретными пакетами Editor/Player и записанными результатами. ME-01 согласует документацию; это не повторный native-прогон и не закрытие критерия холодной подготовки.

Обычные native-пиксели остаются на GPU через непрозрачные Core leases и платформенное interop; четыре аренды и фактическое завершение consumer ограничивают повторное использование. В том pre-async снимке ожидание producer completion оставалось синхронным, явные CPU/software/Glow/PNG diagnostics сохраняют readback. [Отчёт продолжения](p0-continuation-20261018.md) привязывает восстановленные обычные Windows-исходники `cedf173e70f531c8f1784a14ec02830a4aea905f7ff0ebd3aea5a0a07484a6cb` и конкретные артефакты: первая допустимая fresh cold-попытка **40,906 с wall / 39,9128549 с internal**, неизменённые текущие ARPG-критерии (Editor 119,05/Player 119,00 FPS), Windows22/16 и отдельно привязанные текущие WSL Linux23/16 offscreen scenarios/exact comparisons, ограниченные Windows authoring/export/relocated Player 120-кадровые доказательства. Одна cold-попытка не гарантирует время запуска; прежние неудачные cold-попытки остаются неудачными с неизвестной причиной. Диагностические traces и admission-limit1 удалены; production default2 и исходный runtime сохранены.

В том историческом прогоне WSLg/X11 resized Editor и чистый обычный WM_DELETE close проходят. Synthetic Linux pre-submit release-failure pair проверяет ожидаемые production error и quarantine без CPU fallback; это не реальная аппаратная потеря устройства и не hardware Linux support. Прежний close timeout показал Unsaved Changes migration guard в generated fixture; неудачные fixture/cleanup receipts сохранены. Native Linux audio/dialog services, physical input/scanout и произвольная ABI/driver portability остаются вне этих результатов. [Исходный GPU-отчёт](gpu-presentation-20261018.md) сохраняет исторические source15 A/B, source11 Linux и cold-неудачи. Итоговая привязка исходников и независимый вывод находятся во внешних записях продолжения; полная P0 или whole-workspace acceptance не заявляются. Исторический P0 от17 октября ниже не изменён.

### P0: принят в описанных границах — 17 октября 2026

**Проверки P0 и независимое ревью завершены в описанных границах.** [Итоговый отчёт с доказательствами](p0-stabilization-20261017.md) связывает Windows runtime `b260549403e638653b75e880598b507d7965164b11096399cb18006eadbdab2c` и Linux Scene-тесты `94c60e636ca77ae19609e723913a745205da950305210236a60d8033e62e8c70` с фактическими artifacts. SHA256 обычного Editor — `de7ae73680f0c86d1ae1b33fa7de313ecffab1589906b72551debea593e2a9e9`; собственного standalone Player — `27b8ca7c7486a24b7f2df8c1e0927eb61b0a52881c23b77d078f990709c47427`.

Неизменённый 1080p/20-живых-врагов/Playing-критерий проходит: Editor 119.922177561 FPS, worker mean 8.338741176/p95 9.3295 мс; Player 117.082020875 FPS, worker mean 8.541021008/p95 9.2637 мс. Callback p95 — 7.5085/7.6059 мс, UI-observed unique p95 — 13.8867/14.0195 мс, это разные ряды. В каждом отчёте 120 различных завершённых presentations; origins 60/60 и 61/59 committed/interpolated, game/wall ratios 1.00535937/0.99658117, ноль frame/wait errors. Focused-frame counts 145/146 при нуле user-input frames не доказывают held input или физическую scanout latency. Симуляция остаётся фиксированной на 60 Гц; допустимое представление нацелено на 120 Гц без гарантии измеренной частоты.

Target-cold одной попытки проходит оба таймера <=45 с: 37.14 с wall/36.1642576 с internal, с новым процессом, свежим Full03 и ранее отсутствовавшим пустым `work/pcB260-03`. Существующие tools/offline registry и неконтролируемые OS caches сохранены. Фактические [native/relocation](../../outputs/p0-stabilization-20261017/scene-input-fixed-arpg-relocation-01/result.json), [cold](../../outputs/p0-stabilization-20261017/final-cold-editor-03/launch.json) и [FG04](../../outputs/p0-stabilization-20261017/foreground-current-04/result.json) receipts сохраняют source/protected data.

Исходный Scene input red 1-pass/2-fail сохранён; исправленный native-набор проходит все 16 тестов на Windows/Linux. Реальные FG04 Move/один Undo/Rotate/Scale/Save меняют ровно два выбранных transform leaves (Y rotation `0.9277253`, Y scale `1.0593712`); Editor закрывается с exit 0 и явно авторской транскрипцией tool receipt. Шесть captures имеют JPEG bytes несмотря на имена `.png`. Ранний ограниченный FG02 подтверждает Unicode SaveAs, menus/Cancel/Assets/Play guards/history. Generic graph/DragValue drag, held keys/release, MMB/Shift+MMB, Ctrl-wheel и mid-held Escape физически не проверялись. Editor открывается только для коротких конкретных UI-тестов и сразу закрывается.

Применимые functional/native/package-доказательства, шесть обычных CLI cases, строгие Windows checks/Rustdoc, поддерживаемый Full stage и ограниченный Linux WSLg/X11 software-Vulkan Puzzle/Team/перемещённый Player записаны в полном отчёте с каждой source identity. Функциональный Linux `target_met=false` не является Linux ARPG FPS/cold/hardware/Wayland-приёмкой. Default-покрытие — логическое объединение разных snapshots: **Windows 666 ordinary/9 ignored, Linux 664/8**; [unique-name ledger](../../work/verification/p0-stabilization-20261017/composite-test-inventory-scene.json) добавляет только девять новых Scene names/платформу, не дублированные 16 passes или all-feature overlap. Это не один успешный текущий full-workspace запуск. Failed guards/cache fixtures/race/observers остаются failed reports с ограниченными supplements/reruns; все 33 protected files и исходные пользовательские данные сохранены.

[Независимый итоговый decision record](../../outputs/reviews/p0-final-20261017/review.md) связывает итоговый снимок исходников и документации отдельно от неизменённых native inputs. Native Linux WAV/dialog gaps, hardware/ABI support limits, physical input/scanout certification и недоверенный Rust без sandbox остаются явными границами; будущие portfolio systems — отдельная работа.

## Как выбирать доказательства

Последний code snapshot исправлений диалогов — `0fbfb9ca74a139253b4e9d611b2b8877cfb4166d`: [отчёт](dialog-path-fixes-20261016.md) относится к трём изменённым Rust-файлам, Editor tests и перечисленным native сценариям. Полный workspace Windows/Linux из [предыдущего authoring-этапа](remaining-authoring-fixes-20261016.md) относится к `fb63b3ddae7cad52532761932855b755c61fb2c7`; его числа нельзя объявлять новым полным прогоном текущих исходников. Performance-результаты из [review-fixes](review-fixes-20261016.md) остаются историческими; текущий P0-отчёт фиксирует последующие привязанные к исходникам измерения.

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
