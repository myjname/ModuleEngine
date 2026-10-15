# Full, Team и отдельный Player

Full source package предназначен автору движка; Team — автору игры с выбранной архитектурой. Обе поставки содержат редактор. Player — отдельный runtime экспорт без редакторских окон, SDK и компиляторов; его формат и запуск описаны в [player.md](player.md). Доступность исходников отделена от доступности компилятора.

.module-engine-package.json содержит schema_version:1, engine_version:0.4.0, profile, files, module_sources, sdk_files, backend_files и для Team gameplay_source_ids/blueprint_hash. files — relative path → SHA-256 baseline native источников. Проверяются подключённые узлы и engine.enabled_modules.

Engine доступен только Full с полным Cargo/native Rust/HLSL inventory и совпадающей workspace.package.version. Два Cargo/lib файла не заменяют source package. Изменённые исходники отмечаются modified и остаются доступными. Отсутствие DXC/Rust диагностируется отдельно; docs/examples/tools не определяют SourceStatus.available. --seal-source-project <folder> обновляет baseline сознательно; экспорт делает это сам.

Compile-time original workspace не выдаёт авторский доступ. Выбирается marker около executable/проекта или текущего каталога. Team marker имеет приоритет и отвергает --profile full. .sdk без marker отклоняется без заимствования исходников исходной установки.

## Frozen blueprint

Hash фиксирует граф Engine, bindings/source IDs, exposure/ranges/варианты, shaders, module pins, граф исполнения игровых систем и Engine Rust. Разрешённый Rust — подключённые custom.rust.update исходники, включая игровые системы; legacy scalar speed принадлежит Engine. Source key set и привязки неизменны. Позиции/label обычных Graph nodes и code_layout/code_layout_keys gameplay не входят в hash. В ExecutionGraph нет координат: его стадии, узлы, параметры, after и bindings заморожены для Team. Авторские сцены и активные шаблоны остаются редактируемыми игровыми данными.

Project settings, свойства и gameplay текст редактируются в пределах schema. Core проверяет ranges, участие и команды; UI/CLI проверяют blueprint при загрузке/применении/сохранении. Локальный detach сохраняет archive для undo/attach, но runtime его не видит. Team и Player используют общий runtime projection: удаляют неподключённые рабочие узлы рекурсивно, сохраняют обязательные interface mappings вложенных графов и все запланированные игровые системы. Подключённые выключенные проходы остаются доступными для разрешённого переключения. Неиспользуемые рабочие Rust/HLSL sources исключаются до поиска include-файлов; Team сохраняет объявленную библиотеку игровых ассетов, а Player выбирает ресурсы своей игры.

Исходный документ проверяется до projection. Published Engine profile, его архивный blueprint, enabled modules, module versions и exact locks сохраняются без перепубликации. Team stamp закрепляет полученный рабочий граф. New Project → Inherit Current наследует именно его и его source maps: архивные узлы профиля не возвращаются в новый рабочий проект. Изменение состава Engine остаётся недоступным Team.

Backend pins строго проверяют executable, tool manifest, DXC/DLL/лицензии, assets/gpu и рекурсивные literal HLSL includes, включая заголовки вне assets. Обычные game assets не заморожены этой картой: их AssetManifest задаёт paths/hashes.

Это рабочая политика редактирования, не security sandbox и не криптографически подписанная поставка. Пользователь с доступом к файлам может модифицировать программу; Custom Rust имеет обычные права ОС.

## Экспорт и SDK

Full экспорт — весь workspace с новым manifest. Team содержит:

~~~text
ModuleEngine.exe
project.json
.module-engine-package.json
toolchain-manifest.json
run.ps1
scripts/bootstrap.ps1
assets/gpu/...
tools/dxc/... DLL + licenses
.sdk/Cargo.toml
.sdk/Cargo.lock
.sdk/crates/engine-core/...
~~~

.sdk содержит Core контракт для compiled Custom, без native editor/Vulkan authoring пакетов. SDK lock нормализуется из полного Cargo.lock и проверяется как подмножество тех же name/version/source. Compiler берёт tools/cache из package root, sources из .sdk. Неполный SDK даёт ошибку без fallback к original E. Явный Team профиль на Full также требует собственного SDK для compiled Custom.

Экспорт scoped к выбранному Full source/resource root; registered assets разрешаются относительно открытого JSON. Нельзя писать внутрь копируемых деревьев/поверх непустого каталога. project.json, marker, executable, .sdk и cache пути зарезервированы. Неполный результат при ошибке сохраняется для диагностики.

~~~powershell
.\scripts\bootstrap.ps1 -Json
.\scripts\bootstrap.ps1 -Restore
.\ModuleEngine.exe
~~~

Restore выбирает Cargo.toml Full либо .sdk/Cargo.toml Team. Portable Rust лежит в .tools, dependency cache в .work/cargo-home. Требуются host MSVC/Windows SDK и GPU driver; их script не устанавливает. Restore допускает сеть; полного offline toolchain SDK нет.




Исторически проверено в 0.3: автоматический Full/Team export с manifest, relocated Team execution и чтение relative game asset рабочим Rust worker; реальные source/profile случаи complete, modified, missing native/tool, Team policy и blueprint отказ. После первого отказа sandbox DNS повторный сетевой Restore с разрешённым доступом скачал 13 закреплённых Cargo-зависимостей в собственный новый Team-кеш. Затем пакет с очищенными CARGO_HOME/CARGO_TARGET_DIR собрал Custom SDK и отрендерил сцену. Это проверка загрузки зависимостей с уже установленными Rust/MSVC/драйвером, а не холодной установки всей toolchain или полноценного offline SDK. Результаты нового игрового 0.4 этапа публикуются отдельно в [Player](player.md) и [game verification](game-verification.md).
