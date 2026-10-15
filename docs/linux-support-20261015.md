# Подготовка Linux: проверка 15.10.2026

Ветка: `codex/linux-native-preparation`. Исходный commit: `d9eb584f8ee94d152a5a4c0d23a1a4391e32668e`; начальное дерево чистое. Это отчёт исполнителя; независимое ревью будет отдельным этапом.

Linux Editor и экспортированный Player собраны нативно в существующей Ubuntu/WSL2. Реальные Wayland/X11 окна используют WGPU/Vulkan. Player, Full и Team экспортированы; Player проверен без Rust/DXC. Полной готовности Linux пока нет: общий Editor acceptance с Rust Custom не уложился в 45 с при повторной подготовке после Stop; отсутствуют native WAV и диалоги выбора файлов/папок, аппаратный GPU не проверен. Windows GUI остаётся WGPU/DX12 с прежним исправлением DWM.

## Изменения

| Этап | Реализовано |
| --- | --- |
| 0 | Сохранены source seal, Git snapshot и SHA-256 авторских графов, личной раскладки и опубликованного Windows EXE; проверена существующая Ubuntu/WSL. |
| 1 | Общая GUI policy: Windows WGPU/DX12, Linux WGPU/Vulkan. Target-only Wayland/X11 features, выбор оконной системы и actual adapter/display диагностика. Скрытого переключения на Glow нет. |
| 2 | Native имена/platform metadata, Unix execute bits, native DXC/shared libraries, отдельные build caches/lease и Linux no-replace publication. Явная проверка audio capability. Rust Custom/Cargo используют собственные Linux process groups. |
| 3 | Read-only probe создаёт настоящий Vulkan instance, перечисляет устройства, проверяет инструменты/display sockets. Далее выполнены native сборка, HLSL compilation, запуск окон и экспорт пакетов. |
| 4 | Повторены Windows проверки кода, GameWorker, native Editor и экспортированного Player. Glow baseline и изменения драйверов/системных настроек не выполнялись. |
| 5 | Добавлены `run.sh`, `verify.sh`, metadata инструментов и документация; Full/Team переносят новые инструкции/скрипты. |

Публичный SDK и схемы графов не менялись. Метка `native_platform` необязательна для старых schema2 Player manifests; новые метки проверяются до создания Host.

## Среда и установка

Ubuntu 24.04.1 LTS, WSL2, kernel `5.15.153.1-microsoft-standard-WSL2`, x86_64. Единственное Vulkan-устройство — **llvmpipe LLVM 17.0.6**, программный рендерер. Физический Linux и аппаратный Linux Vulkan не проверены.

Изначально отсутствовали Rust/Cargo, C-компилятор, pkg-config и native DXC. После отдельного разрешения установлены Rust/Cargo 1.99.0 с rustfmt/Clippy, GCC 13.3, pkg-config 1.8.1, необходимые Wayland/X11/xkbcommon dev libraries и DXC v1.9.2609. Установлены 68 Ubuntu packages, обновлены 11 их зависимостей; 238 прочих обновлений не применялись. Обновления включают libc/dpkg/compiler runtime/X11. Ядро, Mesa/Vulkan/GPU-драйверы не обновлялись; профили оболочки и глобальные security/environment settings не менялись.

Точные версии, официальные URL и SHA-256 Rust/DXC: `scripts/linux-toolchain.json`. История установки/package diff: `outputs/linux-support-20261015/provision-summary.json`, `provision-packages-installed-or-updated.tsv`. Metadata не запускает installer автоматически.

Локальная активация: `. .tools/linux-x86_64/env.sh`. Native Cargo/rustup изолированы от Windows; target на ext4 в `/root/.cache/module-engine-linux-x86_64/target`. Cargo registry — в отдельном Linux home проекта. Зависимости Cargo загружены явно; существующие закреплённые версии не обновлены. Windows DXC сохранён.

Для дочерних WSLg процессов использован `XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir`: исходный `/run/user/0` не указывал на действующий Wayland socket. Wayland/X11 запускались отдельно, глобальная конфигурация не редактировалась.

## Проверки Linux

Все упомянутые логи: `outputs/linux-support-20261015/`. Команды повторения: [linux-support.md](linux-support.md).

Итог Cargo suites по всем 13 crates после исправлений и точечных повторов: **535 уникальных тестов прошли, 0 failed cases в этой сводке, 1 ignored**. Ignored — отдельный ручной Host catch-up diagnostic; ранее ignored GameUI acceptance выполнен явно и прошёл. Это объединение результатов по именам тестов, не успешный единый первоначальный запуск. Отдельный native Editor benchmark описан ниже и остаётся непройденным. Машинная сводка сохраняет историю попыток: `linux-unit-verification.json`.

* `linux-workspace-check.log`, `linux-editor-release-build.log`, `linux-workspace-clippy.log`: Release workspace/all-targets check, Editor/examples build и Clippy с `-D warnings` прошли.
* `linux-runtime-verification.json`: 20 runtime, 6 host-service, 4 compiler-discovery и 1 native DXC contract test прошли. DXC создал три новых SPIR-V артефакта в отдельной папке, без Windows shader-cache hit. Проверены cleanup потомков, cancel, persistent state/hot apply и native compiler discovery. Повтор native-cache случая не считается отдельным тестом.
* `linux-binding-and-delivery-tests.log`: 9 distribution tests прошли, включая Full/Team ограничения и read-only WSL binding.
* `linux-probe-wayland.log`, `linux-probe-x11.log`: реальные окна, actual WGPU/Vulkan/llvmpipe и соответствующий raw display handle. `linux-headless-cpu.log`: self-check без display variables; snapshots/RGBA двух моделей мира совпали.
* `linux-full-source-export-wsl-fixed.log`, `linux-player-export.log`, `linux-team-export.log`: Full, native Player и Team экспортированы. `linux-team-capabilities.log`: `profile=team`, `engine_available=false`, compile root — `.sdk`.
* `linux-delivery-verification.json`: Player/worker — ELF с mode 0755, platform linux-x86_64; 14 sealed files, без SDK/compiler/sources/cache/symlinks. Team Editor/Player/DXC исполняемы, `run.sh` 0755. Full исходники не ссылаются symlink на checkout.
* `linux-player-precompiled-verify.log`: native Player прошёл с `PATH=/nonexistent`, отсутствующим DXC и без `LD_LIBRARY_PATH`; Rust/DXC во время запуска не требовались.
* `linux-player-relocated-verify.log`: тот же пакет перемещён целиком в `relocated-player` и повторно прошёл из `/tmp`, с теми же отсутствующими инструментами. Начальный путь в `linux-delivery-verification.json` относится к проверке до перемещения.
* `linux-team-custom-check.log`: Team Editor действительно скомпилировал игровую логику через поставленный `.sdk`; Engine остаётся недоступен в Team. Это проверка рабочего SDK, не только наличия файлов.
* `linux-player-wayland/native-frame-report.json`: 1920×1080, 120 различных готовых Game frames, 0 ошибок, actual Wayland/WGPU/Vulkan. **26.81 новых игровых кадров/с**, `target_met=false`. Это cadence готовых worker frames, не физический scanout и не аппаратный GPU benchmark.
* `linux-game-worker/game-worker-report.json`: `passed=true`, source unchanged. Реальный медленный tick 86.07 мс; busy client burst 0.065 мс, drop 0.119 мс. Проверены bounded queue, pause/restart, stale-result rejection, fault latch/recovery. Это стоимость неблокирующего интерфейса, не FPS.
* Финальные `linux-runtime-placement-tests.log`, `linux-source-modules-placement-retest.log`, `linux-owned-custom-placement-retest.log`, `linux-worker-placement-retest.log`, `linux-slow-ui-placement-retest.log`: 22 runtime tests, source component, failure/cancel, оба worker tests и отдельный slow UI прошли. Worker pair — 30.68 с, isolated UI — 19.18 с. IPC-лимит остался 150 мс, задержка Custom — 80 мс. Последние Clippy/doc-test commands и Editor/examples build тоже exit 0; doc-test suites содержат ноль примеров.

Первый общий native unit run остановился на Editor: 188 passed, 6 failed, 1 ignored. Три failure вызваны WAV в Scene fixtures, два — публикацией тестовых пакетов на DrvFS, один — превышением IPC-лимита медленной fixture. После исправления fixtures весь Scene-модуль прошёл (7 tests), весь package-модуль прошёл (9 tests). Все оставшиеся crates также выполнены отдельной последовательностью. Первая ошибка общего запуска и неудачные повторы медленного worker/UI сохранены; они не подменяются успешными логами. Текущий результат каждого теста с устранением дублей хранится в `linux-unit-verification.json`.

Первоначальный `linux-editor-x11/report.json` **не прошёл**: 45 с watchdog, 2588 UI callbacks, Game host/frame готовы, Scene preparation pending. Повтор `linux-editor-x11-warm/report.json` тоже не прошёл: в нём Scene готова, а Game ожидает Custom. Во втором запуске cache оказался подготовлен не полностью: внутри окна измерения пересобирался Full SDK Core. В обоих случаях видны Cargo lock/compile waits, UI callbacks продолжаются; Play/Stop и pan measurements ещё не начинались. Это реальная граница холодного запуска, а не доказанная ошибка Vulkan. Ранняя `linux-editor-wayland.png` показывает подготовку Rust Custom, а не готовую 3D-сцену.

После отдельного последовательного `--check` точного Full SDK (`linux-editor-full-sdk-prewarm.log`, exit 0) запуск `linux-editor-x11-prepared/report.json` подготовил оба окна, выполнил warmup 30/30 и реальные pan samples 90/90. Game time в Play вырос с 0.65 до 2.5 с, identities сохранены, ошибки отсутствуют. Однако **весь acceptance остался красным**: финальный Stop сразу поставил Pause и освободил прежнего владельца, но новый Game запустил новую компиляцию неизменённого controller. До общего watchdog оставалось только 7.83 с; эта сборка была отменена без SDK rebuild или Cargo lock wait. В production нет повторного использования immutable compiled code между новыми Host. Исходный 45-секундный предел не увеличен. Средние Editor handler timings полученных фаз — 2.27/3.12 мс; они не превращают незавершённый общий acceptance в успешный.

Отдельная диагностика точного controller/harness сравнила default debug=2 и `line-tables-only`, с отдельным прогревом SDK/dependencies каждого профиля. Один fresh worker: 9.9345 против 9.3151 с; ELF — 39,669,624 против 14,978,784 bytes. `addr2line` разрешает реальные строки update/invoke в обоих ELF, SDK совпадают. Выигрыш около 0.62 с в одном сравнении не решает бюджет Stop; production profile не менялся. Это отрицательный результат гипотезы об основной роли DWARF, а не успешная оптимизация. Полный отчёт: `linux-controller-debug-diagnostic-1792100611731523730.json`.

Для разделения native GUI/lifecycle и компиляции создана отдельная `project-no-custom.json` в том же проверочном Full: удалён только scheduled node `game-controller`, сцены, root graph, shader/code texts, ресурсы, SDK и пороги сохранены. `linux-editor-no-custom-fixture.json` и `.diff` фиксируют изменение и неизменность исходного Full project. **Этот отдельный benchmark прошёл**: `linux-editor-x11-no-custom/report.json`, complete/measurements_valid=true, 5.9407 с, реальные pan samples 90/90, Play продвигает simulation, Stop немедленно освобождает прежнего владельца и создаёт нового actor 2 при paused/time=0. Errors пусты. Это доказывает native GUI и цикл Play/Stop без scheduled Custom; исходный полный Custom acceptance остаётся красным.

Acceptance использует отдельный Full export на ext4: `/root/.cache/module-engine-linux-x86_64/acceptance-20261015/full`. Только в его `project.json` явно очищены audio cues; diff/hash — `linux-silent-fixture.json`. Авторский проект не изменён. Единственная cache-only symlink ведёт в общий native Cargo target; Cargo пересобрал выбранный Player из экспортированных исходников. Это проверка экспорта с повторным использованием cache, а не холодное восстановление на новой машине.

## Проверки Windows

Первоначальный общий Windows unit run остановился на двух runtime fixtures: junction получил Access denied в sandbox и прошёл нативно; проверка артефактов ошибочно искала `debug/deps` в Release и теперь использует папку текущего test executable. Точечные повторы исправленных случаев и ещё не выполненные crates прошли. Отдельно прошли Vulkan suite, isolated Custom 80 мс UI test, doc-test команды, strict Clippy и rustdoc. Doc-test suites содержат ноль примеров и не дают содержательной проверки документации.

`windows-editor-native/report.json`: 90 Stop + 90 Play pan callbacks, Play/Stop через общий production dispatcher проверены, документ не изменён. `windows-player-native/native-frame-report.json`: 120 новых кадров, 1080p, 0 ошибок, 59.97 FPS, `target_met=false`. Actual GUI adapter — DX12/RTX 4080 SUPER. Окна запускались со скрытым startup window; физический показ/latency дисплея не измерялись.

`windows-game-worker/game-worker-report.json`:passed, tick 83.56 мс, busy client burst 0.104 мс, drop 0.118 мс. Game simulation уже отделена от UI через GameWorker; этот перенос повторно не делался.

После последнего изменения runtime повторены все 21 Windows runtime tests, fmt, workspace/all-targets check, strict Clippy и Editor Release build: `windows-post-placement-*.log`, все exit 0. Начальная попытка проверочной PowerShell-обвязки остановилась на обычном Cargo stderr; исправлена обвязка, результаты Cargo оцениваются по exit code. Это не ошибка теста runtime.

Опубликованный `outputs/ModuleEngine.exe` намеренно сохранён побайтно как проверенный DX12/DWM build; новая Windows source-сборка находится в `target/release/module-engine-editor.exe`. Авторские графы, binding, личные раскладки и прежние Windows DXC проверяются по исходным SHA-256 в `final-preservation.json`. Source seal `.module-engine-package.json` обновлён отдельно от авторских данных.

Финальный `--capabilities game-project-v4.json` подтверждает Full/Engine available, отсутствие source diagnostics и modified sources. Старый корневой `project.json` ожидаемо отклонён CLI как формат 3 при требуемом 4; автоматическая миграция/перезапись не выполнялась. Это ограничение исходного проекта отмечено в инструкции запуска.

## Найденные проблемы и исправления

1. **Windows binding в WSL checkout.** Первый Full export отклонил `\\?\E:\ModuleEngine`. Добавлено строгое read-only разрешение absolute drive path через `/usr/bin/wslpath -u`, без shell/записи sidecar. После canonicalize сохраняются проверки выбранного Engine/manifest/SDK/Team. UNC/device/relative paths отклоняются. Повтор реального экспорта прошёл; binding сохранён.
2. **Unit autosave затрагивал личный layout.** Исходник восстановлен по baseline SHA-256 после сохранения изменённой версии. Механизм и совпадение содержимого подтверждены; точный последний writer не трассировался. Тестовые Editor теперь имеют отдельные Layout/Recent roots. Explicit save/load, per-instance isolation и исходный autosave regression прошли; production paths не менялись. Подробности: `layout-side-effect.json`.
3. **Платформенные fixtures.** Linux тесты явно не запрашивают отсутствующий WAV; Windows audio asserts сохранены. Production demo не изменён и Linux audio не имитируется. Custom использует явно выбранный native Cargo home вместо Windows cache/toolchain.
4. **DrvFS и атомарная публикация.** Direct test binary подтвердил `EINVAL (22)` при `renameat2(RENAME_NOREPLACE)` на общем `/mnt/e`, даже для свободного назначения; проверки отказа при гонке уже проходили. Linux fixtures перенесены в native `TMPDIR`; production сохраняет запрет перезаписи и теперь сообщает об отсутствии поддержки filesystem/kernel. На ext4 экспорт проходит. Лог: `linux-package-publication-drvfs-repro.log`.
5. **Медленный запуск ELF с DrvFS.** 80-мс Custom превышал общий IPC-лимит 150 мс и сохранял fault до Restart. Generic helper затем ещё 12 с ждал ACK, скрывая причину; теперь ошибка включает bounded fault, tick/epoch/ACK и timings. Контролируемая матрица из 80 обменов в 20 новых процессах с одинаковым ELF дала первый ответ за 137–143 мс на DrvFS и 91–94 мс на ext4. Размещение marker влияло примерно на 3 мс; основной вклад давало размещение executable. У измерительной матрицы был отдельно обозначенный лимит 2 с для наблюдения, она не считается прохождением acceptance. Лог/матрица: `linux-worker-phase-diagnostic.json`.
6. **Размещение и владение Linux worker.** Runnable copy теперь хранится в уникальном leaf выбранного native worker target; источники/lock/log остаются в проекте. При последнем освобождении удаляется только принадлежащий кандидату executable, после завершения процесса. Исправлен также пропуск Cargo ELF без суффикса при очистке собственных build outputs. Проверены сохранение общего cache/соседей, shared leases и вложенный target. Windows placement, SDK, IPC-лимит 150 мс и задержка fixture 80 мс не менялись. Исходные красные source/failure/build/marker сохранены в `worker-80ms-full-suite-fault/`, `worker-80ms-paired-retest-fault/` и `slow-ui-80ms-isolated-fault/`.
7. **Инструкции и probe.** CLI-пример теперь учитывает заданный `CARGO_TARGET_DIR`. `verify.sh` честно описывает свои Vulkan/offscreen tests и отдельно требует DXC; проверка native desktop остаётся отдельной. Python probe отвергает Windows PE/interop и неисполняемый DXC до запуска. Syntax, отрицательные случаи и настоящий native `dxc --version` прошли без Cargo/GPU: `linux-script-audit-tests.json`.

## Остаточные ограничения

* Native WAV отсутствует: активный запрос/экспорт выдаёт ошибку. Silent check не доказывает звук; нужен Linux audio backend.
* Native File/Folder dialogs пока только Windows. CLI open, save текущего файла и CLI export доступны; интерактивные Import/Open/Save As требуют native Linux picker.
* Полный native Editor acceptance с Rust Custom не уложился в общий watchdog 45 с: новый Host после Stop заново компилирует неизменённый controller. Оба pan-участка и Play состоялись, мгновенный Stop освободил прежнего владельца, но готовность новой остановленной копии в этом запуске не подтверждена. Нужен отдельный механизм повторного использования compiled code с независимым состоянием и проверкой свежести исходников/SDK; общий тест не объявлен успешным.
* Hardware Vulkan, физический Linux, другие дистрибутивы и ABI/glibc compatibility не проверены. 60 FPS не подтверждены и текущим Windows sample; строгий целевой тест открыт.
* Linux process groups очищают наследующих потомков, но намеренный `setsid` может выйти из группы. Cleanup после аварии host не гарантируется. Forced timeout внешнего verifier/relocation пока гарантирует kill/wait только непосредственного ребёнка. Это lifecycle management, не sandbox.
* Game simulation/рендер работают на одном worker: медленная логика снижает частоту новых кадров. Legacy non-game Engine Runtime ещё имеет синхронный UI-путь.
* Linux Team пока переносит также неиспользуемые Windows DXC exe/DLL (~25.6 MB), поскольку копируется весь `tools/dxc`. Native запускается без них; это лишний размер. DXC mode 0777 отражает DrvFS source; нормализация mode отдельно не добавлена.
* Отдельные известные P2 прежнего ревью — Inspector prefix overrides и inactive-root renderer/subset Player — этим этапом не исправлялись.

Дальше нужны ускорение повторной подготовки Custom после Stop и завершение полного Editor acceptance, native audio/pickers и проверка на аппаратном Linux Vulkan. Полные команды и ограничения сохранены рядом с исходниками.
