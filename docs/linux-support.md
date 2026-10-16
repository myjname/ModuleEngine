# Подготовка Linux и границы проверки

GUI Editor и Player используют общую политику: **Windows — WGPU/DX12, Linux — WGPU/Vulkan**. Игровой рендерер — отдельный Vulkan-модуль. Наличие одного API у GUI не определяет формат игрового графа. Windows-исправление зависаний DWM сохранено; диагностический Glow включается только явно.

Проверки политики с Linux-флагом на Windows не заменяют native сборку и запуск. В существующую Ubuntu/WSL2 по отдельному разрешению установлены native Rust/Cargo, C-компилятор, pkg-config и DXC. [Текущее состояние](current-status.md) связывает завершённые исправления и открытые границы с последующими проверками; [linux-support-20261015.md](linux-support-20261015.md) описывает первый прогон и сохраняется как история.

## Требования к среде

* Native Linux Rust/Cargo 1.99.0 — проверенная версия; более ранние версии в этом прогоне не проверялись. Нужны компоненты rustfmt/Clippy, C-компилятор/linker и pkg-config. Команды ниже не устанавливают инструменты.
* Для GUI: работающая сессия Wayland или X11, runtime библиотеки window system и xkbcommon, Vulkan loader и совместимый ICD. Для разработки/линковки могут понадобиться dev-пакеты выбранного дистрибутива. Их установку нужно выполнить явно средствами своей системы.
* Для нового HLSL: native Linux DXC. Windows `dxc.exe` не используется через Wine/interop. Предкомпилированный Player не требует DXC или Rust, но требует системные runtime библиотеки и GPU-драйвер.
* Native пакеты собираются на своей OS/архитектуре. Windows экспорт не становится Linux-пакетом после переименования EXE. Метка `native_platform` не доказывает совместимость glibc/драйвера со всеми дистрибутивами.

Read-only проверка среды, включая настоящий `vkCreateInstance` и перечисление Vulkan-устройств:

```sh
python3 scripts/linux_prerequisites.py
python3 scripts/linux_prerequisites.py --require build
python3 scripts/linux_prerequisites.py --require gui
python3 scripts/linux_prerequisites.py --require hlsl
```

Код2 означает отсутствие требований выбранного режима. Успех probe не означает, что ModuleEngine собран или окно успешно создано. Тип устройства4/`software=true` означает программный Vulkan; это не измерение скорости дискретной видеокарты. Проверка сокета не доказывает авторизацию X11 или создание Vulkan surface.

## Сборка и запуск

В существующем checkout через native Linux-среду либо в явно созданном Full acceptance-пакете, после подготовки инструментов. Для обычной разработки новый checkout не требуется. Для GUI выберите проект формата 4 без активного запроса WAV; `/path/to/linux-project-v4.json` ниже означает его действительный путь. Корневой `project.json` этого checkout имеет старый формат 3 и сохраняется без автоматической миграции. `game-project-v4.json` имеет актуальный формат, но его аудиозапрос требует явного изменения отдельного проверочного проекта.

```sh
rustc -vV
cargo --version
cargo fetch --locked --target "$(rustc -vV | sed -n 's/^host: //p')"
sh verify.sh
sh run.sh /path/to/linux-project-v4.json
```

`run.sh` и `verify.sh` используют offline Cargo с `Cargo.lock`. Восстановление зависимостей — отдельная явная команда `cargo fetch`. Linux target расположен в `.work/linux-target/<host>`; Windows target и MSVC-настройки не переиспользуются. `CARGO_TARGET_DIR` можно явно заменить. Не подключайте Windows `.tools/env.ps1` в Linux. Windows toolchain-manifest/bootstrap остаются Windows-инструментами; автоматической установки Linux SDK пока нет.

`verify.sh` запускает весь workspace suite, включая настоящий Vulkan rendering и offscreen GameUI-тест, поэтому требует пригодное Vulkan-устройство и native DXC. Это не только CPU/build-проверка. Для проверки одной компиляции достаточно `cargo check --offline --locked --release --target "$(rustc -vV | sed -n 's/^host: //p')" --workspace --all-targets`. Desktop-сессия Wayland/X11 не требуется для offscreen-теста; `--require gui` не используется как его обязательное условие. Создание native окна и экспорт проверяются отдельно ниже.

Полный source export переносит shell-скрипты и Python probe. Экспорт, выполненный на Unix, сохраняет execute bits. Скрипты можно запускать через `sh`, если передача файлов или файловая система потеряла эти биты. Native `Player` должен оставаться исполняемым.

На проверяемом ПК подготовлена отдельная native среда: `. .tools/linux-x86_64/env.sh` в Ubuntu. Этот локальный файл содержит абсолютные пути текущей машины и не поставляется как переносимый SDK. Он задаёт отдельные Linux Cargo/rustup homes и ext4 target; не изменяет Windows `.tools/cargo`, `.tools/rustup` или профили оболочки.

Проверенные версии, официальные URL и SHA-256 Linux Rust/DXC сохранены в `scripts/linux-toolchain.json`; этот файл переносится Full/Team вместе со `scripts/`. Это metadata для явного восстановления, не автоматически исполняемый installer. Windows-манифест и его pins сохранены.

Проверочные CLI после `verify.sh`:

```sh
host=$(rustc -vV | sed -n 's/^host: //p')
editor="${CARGO_TARGET_DIR:-$PWD/.work/linux-target/$host}/$host/release/module-engine-editor"
"$editor" --integration-check outputs/linux-verification/integration
"$editor" --game-worker-check game-project-v4.json outputs/linux-verification/worker
"$editor" --scene-assets-check game-project-v4.json outputs/linux-verification/scene-assets
"$editor" --editor-ui-check game-project-v4.json outputs/linux-verification/workflow
```

Эти команды требуют соответствующих runtime/SDK/DXC. Native Linux acceptance выполнялась для указанных в [отчётах](current-status.md#как-выбирать-доказательства) snapshots; приведённые команды сами по себе не подтверждают текущий HEAD. Acceptance может создавать временные Full/Team/Player-пакеты; запускайте последовательно. Активный запрос неподдерживаемого native audio нужно сначала явно убрать в отдельном проверочном проекте; production-проект автоматически не меняется.

## Wayland, X11 и диагностика

Оконная система выбирается независимо от GUI GPU backend:

```sh
MODULE_ENGINE_WINDOW_SYSTEM=wayland sh run.sh /path/to/linux-project-v4.json
MODULE_ENGINE_WINDOW_SYSTEM=x11 sh run.sh /path/to/linux-project-v4.json
MODULE_ENGINE_WINDOW_SYSTEM=auto sh run.sh /path/to/linux-project-v4.json
```

Auto использует выбор winit: непустой Wayland environment имеет приоритет. После неудачного подключения к Wayland автоматический повтор через X11 не обещается. При ошибке проверьте `WAYLAND_DISPLAY`, `WAYLAND_SOCKET`, `XDG_RUNTIME_DIR` и `DISPLAY` либо явно выберите X11. Неисправная конфигурация сообщает причину; скрытой замены WGPU на Glow нет.

В обнаруженном WSLg `WAYLAND_DISPLAY=wayland-0`, но сокет лежит в `/mnt/wslg/runtime-dir/wayland-0`, а `XDG_RUNTIME_DIR=/run/user/0` на него не указывает. Для отдельного процесса допустим абсолютный путь:

```sh
WAYLAND_DISPLAY=/mnt/wslg/runtime-dir/wayland-0 python3 scripts/linux_prerequisites.py
```

Это адрес именно обнаруженной среды, не универсальная настройка Linux. Глобальные переменные/системные настройки probe не исправляет.

Для запуска самого Editor в этой WSLg использовалось `XDG_RUNTIME_DIR=/mnt/wslg/runtime-dir` только в окружении дочернего процесса. Этот вариант проверен реальным созданием Wayland-окна; X11 отдельно проверен с `MODULE_ENGINE_WINDOW_SYSTEM=x11`.

В общем Windows/WSL checkout привязка проекта может содержать Windows-путь вроде `E:\ModuleEngine` или его verbatim-вариант. На Linux такой абсолютный drive path разрешается через `/usr/bin/wslpath -u`, без shell и без записи в sidecar. Затем выполняются обычные проверки выбранного Engine, manifest, SDK и профиля. UNC/device/относительные Windows-пути не преобразуются. На обычном Linux без `wslpath` нужно явно выбрать доступный native Engine; отказ не разрешает другую архитектуру или доступ к отсутствующим исходникам.

Короткая проверка native окна без проекта, GameHost и сохранения layout:

```sh
host=$(rustc -vV | sed -n 's/^host: //p')
MODULE_ENGINE_PROBE_SECONDS=5 MODULE_ENGINE_WINDOW_SYSTEM=x11 \
  cargo run --offline --locked --release --target "$host" -p module-engine-editor --example desktop_presentation_probe
# Затем отдельно повторить с MODULE_ENGINE_WINDOW_SYSTEM=wayland в исправной сессии.
```

Запуск с `--target` без `CARGO_TARGET_DIR` использует стандартный `target/<host>`; при необходимости задайте ту же переменную, что в `run.sh`. Не запускайте одновременно несколько GPU-проверок.

Логи различают `requested` параметры и фактически `initialized` adapter/backend/window-system. `WGPU_BACKEND` не переопределяет Windows DX12 или Linux Vulkan. `MODULE_ENGINE_GUI_BACKEND=glow` — явная диагностика, требующая отдельного решения на машине с прежним DWM-дефектом. `EFRAME_SCREENSHOT_TO` больше не включает Glow неявно: для Editor используйте `MODULE_ENGINE_UI_SCREENSHOT_TO` с WGPU. Исторический DWM-отчёт описывает прежнее поведение capture.

## DXC, пакеты и процессы

Поиск DXC: явный `MODULE_ENGINE_DXC`, native tools в backend package, затем `PATH`. Рекомендуемая структура:

```text
tools/dxc/bin/dxc
tools/dxc/lib/libdxcompiler.so
```

Допустим также `tools/dxc/dxc` с `tools/dxc/lib`. Проверяется ELF и execute bit. Соседний `lib` добавляется в `LD_LIBRARY_PATH` только дочернего DXC; окружение редактора/системы не меняется. Пакет должен включать фактические shared-library зависимости своей версии DXC. Список native библиотек внутри Linux-пакета требует проверки на Linux. Подключённый Vulkan Team требует DXC; граф без этого backend не приобретает такую зависимость.

Native Player имеет имя `Player` на Linux и `Player.exe` на Windows. Новые manifests записывают `native_platform=<os>-<arch>` и отвергают чужую платформу до создания Host. Старые schema2 manifests без метки остаются совместимыми; сама метка не заменяет проверку binary/ABI. Схемы графа и публичный SDK не менялись.

Native build/cache на Unix разделён по платформе, сборки сериализуются файловой блокировкой. Linux-публикация Player использует `renameat2(RENAME_NOREPLACE)`; неподдерживающий syscall/файловая система приводит к отказу, а не к неатомарному перезаписыванию назначения.

В проверенной WSL общий Windows-диск `/mnt/e` (DrvFS) возвращает `EINVAL` для этой операции даже при свободном назначении. Поэтому публикация Player туда отклоняется с диагностикой; используйте назначение на native Linux filesystem. Экспорт на ext4 проверен. Исходники/ресурсы при этом могут находиться на Windows-диске — требование относится к файловой системе назначения, где создаётся staging и выполняется публикация.

Экспорт выбранного Player использует собственный cache `<Engine>/.work/player-native-target/linux-x86_64`, отдельно от `CARGO_TARGET_DIR` сборки Editor. Поэтому первый экспорт может повторно собирать GUI-зависимости даже после готовой сборки редактора. В проверочном Full-пакете явно переиспользован только build cache через symlink; это описано в отчёте и не означает холодную сборку на чистой машине.

Rust Custom использует worker target `custom-workers` внутри выбранного `CARGO_TARGET_DIR`. На Linux готовый worker запускается из уникального каталога `executables/<candidate>/worker` в этом target; исходники, lock и диагностика остаются в исходном рабочем каталоге проекта. Последний владелец закрывает процесс и освобождает только свой executable; общий cache и соседние кандидаты сохраняются. Для target, вложенного непосредственно в каталог кандидата, worker остаётся рядом с исходниками, вне удаляемых build outputs. Windows placement сохранён.

В общем Windows/WSL checkout выбирайте native ext4 для `CARGO_TARGET_DIR` и `TMPDIR`. В измеренном случае запуск ELF с DrvFS добавлял около 48 мс к первому ответу Custom и приводил к превышению неизменённого IPC-лимита 150 мс у кода с задержкой 80 мс. Размещение executable на ext4 устраняет эту измеренную причину; оно не гарантирует выполнение произвольного пользовательского кода за 150 мс.

При включённом [compiled-program store](custom-program-store.md) native filesystem нужен также для установленной toolchain и Cargo registry: проверка читает содержимое compiler/link inputs, а не доверяет только mtime. В проверке 16.10.2026 один полный proof с Rust/Cargo на `/mnt/e` занял 102,04 с. После точного переноса закреплённой установки в native cache Ubuntu с проверкой bytes/modes/symlinks тот же production context прошёл за 2,26 с. Это устранение измеренного IO bottleneck, не обещание времени для любого проекта; версии, watchdog и проверка входов сохранены. Условия, пути и остальные native результаты находятся в [отчёте этапа](review-fixes-20261016.md).

Процессы компиляции и исполнения Rust Custom на Linux, включая Cargo этой подсистемы, принадлежат отдельным process groups. Отмена и завершение сигнализируют наследующим потомкам и ожидают лидера; `waitid(WNOWAIT)` сохраняет PID до завершения группы. Намеренный `setsid` может выйти из группы; гарантия очистки после аварии самого host не добавлена. Это управление жизненным циклом, не sandbox. Windows Job Object сохранён. Принудительный timeout внешнего verifier/relocation-процесса пока убивает только непосредственного ребёнка; полная гарантия его descendant cleanup остаётся ограничением.

Повторное использование готового scheduled Rust Custom после Stop реализовано в [compiled-program store](custom-program-store.md). При неизменённом supported compile context экземпляры используют один проверенный код, сохраняя независимые state и worker PID; Cargo freshness proof не означает новую root compilation. Это подтверждено последующими [Windows/Linux проверками](remaining-authoring-fixes-20261016.md), а прежний отказ 45-секундного прогона остаётся историческим результатом. Холодная подготовка Editor в пределах 45 секунд и строгая Player 60 FPS цель всё ещё открыты; успешный prewarmed benchmark их не закрывает. См. [текущее состояние](current-status.md).

## Опциональные сервисы

Native WAV реализован на Windows. На Linux активный запрос `audio.wav` выдаёт ошибку до загрузки ассетов; export также отклоняется. Отсутствующий/отключённый service допустим. Экспорт не удаляет звук молча и не меняет авторские данные. Нельзя считать прошедший silent/headless check доказательством работы звука.

Для проверки Linux GUI/экспорта используйте проект, явно не запрашивающий WAV: например Puzzle или собственный профиль без audio. Текущие ARPG-настройки со звуковыми cues требуют явного изменения отдельной копии проекта. Полноценный Linux audio backend — следующий отдельный этап.

Native File/Folder dialogs пока реализованы только на Windows. На Linux команды выбора файла/папки сообщают об этом ограничении. Открытие графа по CLI-пути, сохранение в уже выбранный файл и CLI-экспорт не зависят от этих диалогов; интерактивные Import/Open/Save As требуют следующего этапа. Проверка запуска окна не доказывает parity всех меню с Windows.
