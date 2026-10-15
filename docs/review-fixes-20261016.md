# Исправления ревью и повторной подготовки Custom — 16.10.2026

Работа выполняется. Проверенная исходная база — `b4f099b51a2cb37bc007a678b355d775c34935e3`, новая ветка `codex/review-fixes-and-custom-reuse`. Исходное tracked tree чистое. Свидетельства ревью в `outputs/independent-audit/linux-review-b4f099b/` сохраняются; исправления выполняются в основном checkout, не в архиве ревью.

## Границы и порядок

1. Asset mutation: только типизированные AssetReference и встроенные resource contracts; Text и неизвестные архивные payload сохраняются. Проверяются реальные move/delete/relink, validation, сохранение и rollback.
2. Inspector: одинаковое longest-owner правило для prefab/local/unknown attachments; Apply/Inherit одного компонента сохраняют данные соседнего namespace. Проверяется production edit path и reopen.
3. Custom build: фактический Cargo executable и native target/config; Windows/Linux default, explicit native env и config, foreign target отказ, cleanup/cancel/leases.
4. Player/Team: единая runtime closure до выбора features, sources и resources; software-active/Vulkan-inactive export, явные enabled modules, активный Vulkan, transitive includes и перенос Player.
5. Custom reuse: ограниченное хранение неизменяемого compiled code, отдельные instance/state/worker. Freshness учитывает source/SDK/dependencies/lock/toolchain/target/build configuration; сомнение означает safe miss. Главная приёмка — полный исходный native Editor benchmark с Custom и прежним общим watchdog 45 с.
6. Общие проверки финального source, реальные Windows DX12/Linux Vulkan окна, обновление готового Windows EXE, preservation, Git и отчёт.

Этапы реализации и focused checks выполняются последовательно. Независимое чтение следующих этапов допустимо. Тяжёлые сборки и GPU/performance проверки не конкурируют за ресурсы во время измерений. Исторические 535/197/158 тестов не считаются проверками новых исправлений.

Windows GUI остаётся WGPU/DX12; Linux GUI — WGPU/Vulkan с Wayland/X11. Glow baseline, драйверы и системные настройки не меняются. ECS/GameSession/shared Host, frozen Object compatibility, optional ARPG и отдельные Two Arenas/Puzzle сохранены. Native audio/pickers, аппаратный Linux/ABI и дополнительные P3 ревью находятся за границами этой задачи.

## Baseline

`outputs/review-fixes-20261016/stage0-baseline.json` содержит HEAD/branch/status, SHA-256 авторских JSON, layout, binding, опубликованного Windows EXE и SDK metadata, Windows toolchain и свободный диск. Предыдущий рабочий EXE сохранён отдельно как `prior-ModuleEngine-DX12.exe`; это один готовый артефакт, не копия исходного проекта.

Linux использует уже установленную native среду `.tools/linux-x86_64/env.sh` и ext4 target `/root/.cache/module-engine-linux-x86_64/target`. Native baseline probes сохраняются отдельно от новых функциональных приёмок. Новые инструменты/ОС/драйверы автоматически не устанавливаются.

## Результаты этапов

### 1. Типизированные операции с ассетами

`AssetWorkspace` получает shared composition через `with_composition`; callback validation больше не считается источником типов. Move/delete/relink/usages работают по явным `AssetReference` и builtin resource contracts. Text сохраняется. Unknown/inactive/stale payload, включая весь inactive property store, остаётся архивом; неоднозначные совпадения возвращаются диагностикой и показываются редактором. При существующем lock отсутствующий или несовместимый schema pin не позволяет переписывать данные. Detach удаляет только действительно отвязанный пустой известный mesh, без глобальной очистки opaque payload.

Windows: `cargo test --offline --locked --release -p module-engine-project --lib` — **50 passed**, лог `stage1-project-final.log`. Project strict Clippy и Editor release all-targets check прошли (`stage1-project-clippy-final.log`, `stage1-editor-check.log`). Исходный public audit repro без изменения его кода теперь возвращает `before=assets/a.png`, `after=assets/a.png` при успешном move (`stage1-public-text-repro.log`). Покрыты настоящие типизированные поля `id`/`module`, nested arrays, prefab/local/overrides, stable IDs, disk save/reopen и отказ до изменения файлов.

Промежуточные красные логи сохранены: Clippy замечание в новом тесте исправлено; source fixture после добавления схемы требовала настоящего `capture_lock`, production pin guard не ослаблялся (`stage1-project-missing-fixture-pins.log`). Linux и общий финальный source пока не проверены заново; успех всего этапа работ ещё не заявляется.

### 2. Владение overrides в Inspector

Общий core helper `component_override_owner` используется разрешением сцены, runtime stripping, asset mutation и Inspector. Apply/Inherit/Remove вычисляют владельца по полному prefab+local набору до удаления компонента. `component_value` также больше не подмешивает override длинного ID в draft короткого. Apply локального компонента очищает его прежние overrides, чтобы они не перекрывали только что введённые значения. Prefix-related ID по-прежнему разрешены.

Windows: production Inspector test кликает настоящие Apply/Inherit кнопки во всех **18 сочетаниях** prefab/local/mixed и active/inactive/unknown длинного компонента; проверяет validation, disk save/reopen, исходный `foo.bar.value`, color и opaque local payload. `stage2-inspector.log` — 5 tests passed; отдельные local Apply/Remove и namespaced vector tests прошли (`stage2-local.log`, `stage2-namespaced.log`). Core **101** и Project **50** прошли вместе (`stage2-core-project.log`). После переиспользования helper Clippy указал на ставшее избыточным замыкание; исправление не меняет семантику, промежуточные логи сохранены. Финальный strict Clippy результат хранится в `stage2-clippy.log`.

Последующее адресное QA нашло два уточнения. При существующем lock требуется также implementation pin владельца, а не только точный SchemaPin. При генерации новых overrides Apply проверяет владельца ещё раз: неизвестное nested поле `foo.bar.value` внутри aggregate `foo` сохраняется локально, если dot path принадлежит отдельному прикреплённому `foo.bar`. Иначе reset сохранял бы чужой override, а последующий Apply снова перезаписывал его. GUI matrix дополнена именно такими aggregate данными и использует опубликованный через production helpers Engine profile и настоящие exact locks; полный document/profile/SDK validator проверяется до/после Apply и после disk reopen.

Уточнения проверены после завершения stage3: Project **51 passed** (`stage12-qa-project.log`), Inspector group **5 passed**, включая 18 комбинаций (`stage12-qa-inspector.log`), strict Editor all-targets Clippy exit0 (`stage12-qa-clippy.log`). Fixture пакетного поиска ресурсов первоначально также не pin-ила новый module owner; она исправлена настоящим `capture_lock`, не ослаблением guard. Красный лог сохранён как `stage12-qa-project-missing-fixture-owner-pin.log`. Исходные зелёные stage2 логи не подменяются поздними результатами.

### 3. Фактические Cargo artifacts и native target

Путь worker берётся из `compiler-artifact` собственного package/manifest/bin, а не собирается как `target/debug/name`. Требуются успешный exit, однозначный executable и `build-finished.success`. Generated std-only build script публикует HOST/TARGET; перед SDK probe проверяются выбранный native Rustc host, равенство HOST=TARGET и PE/ELF machine/header. Cargo environment и конфигурация target сохраняют действие.

Cargo JSON разбирается отдельным streaming collector: общий длинный поток дренируется, при этом ограничены длина строки, число/размер retained products/proofs и rendered diagnostics. Worker protocol packet limit не увеличен. Ошибки Rust возвращаются читаемым rendered текстом; исходный поток сохраняется ограниченным журналом сборки.

Очистка знает default и explicit-native profile scopes, а также подтверждённые собственные build-script products. Canonical path guards и exact package/Cargo suffix matching сохраняют shared dependencies, соседние имена и активные runnable leases. Linux executable placement на native target filesystem из b4 сохранён. Неподтверждённые внешние build directories не удаляются предположительно.

Полный Linux runtime запуск до последних узких уточнений: **32 passed**, 511.43 с (`stage3-linux-runtime.log`). Это функциональные cold-compilation проверки, не performance acceptance. Windows первый полный запуск: 28/29 passed; единственный failure — sandbox отказ `mklink /J` до самого checked cleanup. Exact тест с разрешённым native доступом прошёл (`stage3-windows-junction-native.log`); исходный красный лог сохранён.

Последующие проверки всех затронутых cleanup/native/foreign/collector/diagnostic/stream сценариев прошли на обеих системах, строгий runtime Clippy также прошёл. Суммарное различимое покрытие этапа — **31 Windows / 33 native Linux cases**; это исходные полные запуски плюс адресные повторы, а не один полный запуск финального source. `stage3-summary.json` содержит результаты 11 завершающих команд, SHA-256 трёх implementation files и проверку отсутствия оставшихся Cargo/rustc/test-workers. Ещё один красный Windows fixture лог сохранён: uppercase hash в тесте совпал с собственным lowercase именем на case-insensitive filesystem; fixture заменена отдельным именем, production matcher не ослаблялся. Финальный общий source будет проверяться отдельно после этапов 4/5.
