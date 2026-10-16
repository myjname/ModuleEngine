# Четыре дополнительных P2 после ревью 17fb340

Работа выполняется в ветке `codex/postfix-p2-review-fixes` от `17fb340f29be6cf96563948b5a6c2a57c4e4efaf`. Перед изменениями tracked tree было чистым. Исходный отчёт и его воспроизведения сохранены в `outputs/independent-audit/postfix-review-17fb340/`; новые результаты находятся отдельно в `outputs/postfix-p2-fixes-17fb340/`.

## Границы и сохранность

В работу входят только schema-default asset dependencies, partial-prefab Inspector Apply, имена Windows tools с расширением и actual Cargo products при source-built Player/Team. Дополнительные P3 и архитектурные изменения не включены. Игра, ECS/GameSession, shared Host и опциональность ARPG сохраняются.

`stage0-baseline.json` содержит восемь защищённых SHA: авторские JSON, binding, personal layout, SDK metadata и опубликованный EXE. Копия последнего сохранена как `prior-ModuleEngine-17fb340.exe`. Опубликованный `outputs/ModuleEngine.exe` пока не заменяется. `.claude/settings.local.json` сохранён; этот ignored/untracked metadata файл не входит в изменения продукта.

Используются уже установленные Rust/Cargo/DXC: Windows `.work/cargo-home`, Linux `.tools/linux-x86_64/env.sh`. Новых установок нет. Тяжёлые сборки и GPU-сценарии выполняются последовательно. Windows TEMP короткий — `.work/audit-tmp`. ОС, драйверы, security settings, release profiles и IPC limits не меняются.

## 1. Ресурсы из schema defaults

Раньше прикреплённый компонент `{}` мог эффективно ссылаться на PNG из defaults, но usage index видел только существующие authored pointers. Move/Delete проходили, оставляя effective reference на исчезнувший файл.

Каталог теперь хранит отдельно writable authored references и effective schema-default references с provenance. Учитываются exact owner/schema pins, activity, prefab/local shallow merge, defaults и longest-owner overrides. Usage вида `schema-default:<field> at <location>` указывает на установленную schema, а не на отсутствующее поле authored JSON.

Операция, которая потребовала бы изменить такой immutable default, отклоняется до записи проекта, файлов, history и trash. Сама установленная schema не переписывается. Для изменения пользователь может явно задать соответствующие значения в проектных компонентах/шаблонах; действительная default-зависимость должна исчезнуть из всех затронутых авторских источников. Default вида `asset://ID` допускает перенос файла с сохранением ID. После Detach отдельно проверяется кандидат: удаление authored значения не может незаметно раскрыть default на удаляемый ресурс.

Windows focused: `stage1-project-tests.log` — **26 passed**, включая четыре новые регрессии с настоящими PNG/файловыми операциями. Проверены direct/nested-array/prefab/shallow-local defaults, RefuseUsed/Detach/Replace/Move, ID Move+Undo, overrides и inactive/stale pins. Затем добавлена операция-регрессия допустимого explicit null для local, prefab, leaf override и aggregate override внутри массива: настоящий Core resolution, Move/Delete/Undo, сохранность схемы и авторских данных. Она прошла в свежем полном Windows-прогоне. `stage1-project-clippy.log` — strict release all-targets Clippy exit0.

Гарантия нового default-guard относится к AssetReference-полям активных прикреплённых `ComponentSchema` в `game.scenes`/`game.prefabs` при совместимых exact owner/schema pins. Допустимый явный null сохраняется. Defaults из `ModuleDescriptor.properties` / `PropertySchema` — Global, Module, Object и legacy Component — этим исправлением не разрешаются. Их runtime рассчитывает отдельно через `resolved_properties`, включая фактические live IDs и per-object attachments. Поэтому зависимость, заданная только таким PropertySchema default, пока может отсутствовать в usage index. Существующие явные authored ссылки продолжают обрабатываться по активным контрактам. Это остающееся ограничение, а не защита всех типизированных defaults.

## 2. Apply для частичного prefab

Раньше schema defaults `stats={speed:1,weight:3}` и prefab `stats={speed:2}` давали draft `{speed:2,weight:3}`, но неизменённый Apply записывал local `stats={weight:3}`. По существующему shallow merge это заменяло весь prefab aggregate и возвращало speed1.

Inspector сравнивает draft с эффективным prefab + schema defaults. Неизменённый Apply не создаёт лишних local/override данных. Изменение default-only leaf можно выразить обычным dot override; если требуется local storage, записывается весь верхний aggregate с сохранением соседних значений. Общий shallow-merge contract формата не меняется, ownership `foo`/`foo.bar` сохраняется.

Регрессия через настоящую egui-кнопку Apply проверяет пять случаев: unchanged draft, изменение default-only leaf, неизвестное nested поле, materialization массива с сохранением обоих индексов и optional ObjectReference с явным null. Включены strict validation, save/reopen и сохранность prefab/profile/lock. Все случаи прошли в полном Windows-прогоне. Первые два appfixture-теста отказали из-за stale source inventory, а не Inspector; исходный красный лог сохранён как `stage2-inspector-stale-inventory.log`. После reseal оба прошли в полном прогоне.

## 3. Короткие Windows имена Cargo/Rustc

Кэш больше не превращает `cargo.exe` и `rustc.exe` в `*.exe.exe`: суффикс добавляется только к extensionless имени на Windows. Порядок PATH, явные пути и другие расширения сохраняются. Linux lookup не получает Windows-суффикс. Неподдержанная freshness-конфигурация по-прежнему использует безопасную обычную компиляцию; ошибки compiler/probe не маскируются blanket fallback.

Windows focused: `stage3-cache-context.log` — **3 passed / 2 ignored**; `stage3-exe-public-api.log` проверял первоначальные четыре варианта. Свежий `stage5-windows-native-exe-matrix12.log` проверяет **12** случаев: cargo/rustc × bare/suffixed/absolute × stored/uncached. В каждом реальный worker возвращает ожидаемое значение; настоящий E0425 остаётся Compile error. Это узкая проверка выбранных инструментов, а не обещание полной Windows Command lookup/PATHEXT совместимости. Явный относительный `./cargo` не получает автоматически `.exe`.

## 4. Фактический Cargo executable для Player/Team

Раньше source builder всегда читал `target/release/<player>`, хотя explicit native target выбирает `target/<triple>/release`. На холодном кэше экспорт отвергал успешную сборку; при старом default-layout файле мог выбрать его вместо фактического результата.

Player использует тот же bounded Cargo collector и native executable validation, что Custom. Exact canonical manifest/package/bin выбираются из Cargo metadata, включая virtual workspace без resolve root. `compiler-artifact.executable`, успешный build-finished, HOST=TARGET выбранного native Rustc и PE/ELF проверяются до использования продукта. Никакой fallback к старому default filename не выполняется.

Новый std-only `crates/player/build.rs` публикует HOST/TARGET proof и отвергает foreign target. Файл включён в обязательную native source inventory. Lease исходного builder продолжает охватывать сборку, hash/copy и проверку immutable snapshot. Для большого Player build collector дренирует сообщения зависимостей без хранения ненужных dependency facts; Custom freshness сохраняет прежнее полное ограниченное отслеживание.

`stage4-cargo-products.log` — **9 passed**, включая virtual-workspace selection, large dependency stream, stale default sentinel и invalid header. `stage5-windows-final-player-targets.log` — настоящий Cargo на минимальном Player fixture: default/env/config native target, cold/warm rebuild, unchanged build с явно проверенным `compiler-artifact.fresh=true` и foreign target rejection. `stage5-windows-final-player-snapshots.log` — прямой competing lease test без Cargo перед захватом, затем два конкурентных feature-варианта через production snapshot helper; оба сохранённых executable после завершения возвращают каждый свой результат. Эти tests не подменяют реальный Full/source Player/Team export, который выполняется отдельно.

## Свежая Windows проверка

`stage5-windows-workspace-tests.log`: **595 passed / 0 failed / 9 ignored**, offline locked release workspace/all-targets, один test thread. Doc-tests exit0. Исходные PowerShell5 logs сохранены без изменений; `*.readable.log` — UTF8-производные с отдельно декодированными header/footer и UTF16 native redirection, преобразование и SHA записаны в `stage5-windows-readable-logs.json`.

Первый strict Clippy обнаружил лишнее заимствование `join(&key)` в новом helper. Оно исправлено на `join(key)` без изменения поведения. `stage5-windows-final-{fmt,clippy,build}.log` — exit0 на исправленном source; затем явно прошли оба Player native tests и пять выбранных native tests. Изменение после полного suite затрагивает только это заимствование и усиление двух ignored Player tests; они исполнены отдельно. Полный suite не дублируется без нового основания.

`stage5-windows-native-stop-reuse.log`: после Game root_artifacts_built=1; после инициализации графики появился env key `=C:`, и Scene имеет root_artifacts_built=2. После Stop/Game остаётся2, root_artifacts_fresh=1, program_hits=1: нового root compilation при неизменённом последующем контексте нет. Дополнительный Cargo/freshness invocation не выдаётся за новую root compilation. Проверены независимые worker PID и начальные state. Slow UI/hot Apply test тоже прошёл. Два остальных ignored — генератор software fixture и отдельный wall-time diagnostic медленного catchup — в этот проход не запускались.

Реальная Windows integration завершена: новый Full имеет Engine=true и modified_sources=[]; env native target проверен с изначально отсутствующим executable, затем повторён на тёплом кэше; private Full `.cargo/config.toml` с native target тоже экспортирует Player и Team. Три exported Player совпадают SHA с фактическим текущим `target/x86_64-pc-windows-msvc/release/module-engine-player.exe`. Старый default-layout binary имеет другой SHA и не изменён. PE/machine и каждый package file hash проверены. Все три Player проходят `--verify-package` из другого cwd с ограниченным PATH и недоступным DXC. Team имеет Engine=false, source_root=null, working SDK; его `--check`, экспорт через checked prebuilt Player и compiler-less verify тоже прошли. Сводка — `windows-verification-summary.json`.

Первый native WGPU/DX12 Editor benchmark сохранил **красный** `stage6-windows-editor-dx12/report.json`: обе серии90 кадров были собраны, но cold SDK compilation заняла около30s, а post-Stop preparation не уложилась в существующий45s watchdog. Из этого неполного отчёта не делается успешный performance вывод. После подготовки текущего config тот же benchmark с неизменённым watchdog прошёл: `stage6-windows-editor-dx12-warm/report.json`, complete=true, measurements_valid=true, Stop actor1→2/time0, errors=[]. GUI — WGPU/DX12, NVIDIA RTX4080SUPER. Editor handler p95 stopped/play=2.89/3.94ms, callback interval p95=7.41/7.66ms; это callback timings, не измерение scanout и не Player60FPS. Game-worker diagnostic тоже passed. Cold startup within45s остаётся недоказанным.

Все кодовые исправления сохранены в `b81557f`, `2cf04bc`, `7bcaab2`, `c30ff3b`, `566eafb`. Дальнейшие commits содержат документацию и inventory, если отдельно не указано изменение кода.

Граница Windows GUI evidence: benchmark не имел input focus (`focused_frames=0`); visibility API вернул unknown (`visible=null`), поэтому `visible_frames=0` не доказывает ни скрытое, ни видимое окно. Foreground input-to-display и внешние pointer clicks не подтверждены. Логи подтверждают native WGPU/DX12 event loop, реальные Game/Scene frames, изменения graph pan и lifecycle; manual visual QA/scanout этим не заменяются.

## Свежая Linux проверка

Ubuntu/WSL: `linux-final-workspace-tests.log` — **604 passed / 0 failed / 8 ignored**, offline locked release workspace/all-targets. Doc-tests, fmt, strict release workspace/all-targets Clippy и сборка Editor/Player — exit0 на source `566eafb`. Системные компоненты и профили не менялись.

Первый native Stop/reuse был ошибочно запущен через `cargo test` и отказал на scene root-count assertion; лог `linux-final-stop-reuse.log` и unsuccessful `linux-final-summary.json` сохранены. Сам test предписывает normal native loader environment. Повтор использует именно текущие ELF test executables из `cargo test --no-run --message-format=json`, с проверкой unique target, build-finished, ELF machine и SHA; actual paths/fresh fields сохранены в `linux-native-rest-products.json`. Запуск идёт напрямую в normal environment из существующего env.sh, а не с Cargo-injected loader/test environment. Обычный direct run прошёл без изменения source и условий проверки. Из этого не выводится обещание одинакового caching behavior во всех injected test environments.

`linux-native-rest-status.tsv` содержит шесть exit0: actual context, native tools, Stop/reuse, slow UI/hot Apply, Player default/env/config/fresh/foreign modes и concurrent snapshots/lease. В direct Stop/reuse root_artifacts_built=1 на Game, Scene и Stop/Game; program_hits=2, root_artifacts_fresh=2, три Cargo invocations. State и PID независимы. Два оставшихся ignored — fixture emitter и catchup wall-time diagnostic — не запускались.

Реальная Full/Player/Team/WSLg integration прошла: `linux-integration-all-summary.json` all_passed=true. Перед первым env-target export executable отсутствовал; проверены cold/warm env exports и private Full config-target export. Native ELF/machine62, executable bits и SHA всех package files проверены. Четыре Player, включая экспорт из готового Team, совпадают SHA с текущим Cargo native-target product. Существовавший default-layout executable имеет другой SHA и остаётся неизменным. Все четыре package перенесены в новые папки, затем проверены из `/tmp` с PATH=/nonexistent, недоступным DXC и без LD_LIBRARY_PATH. Artifact manifest фиксирует пути до переноса; фактические новые папки имеют суффикс `-relocated`.

Для этого изменена только новая тестовая копия: её audio_cues явно очищены из-за unsupported native WAV, удалённые cues и SHA original записаны в `linux-integration-silent-fixture.json`. Авторские JSON и scheduled Rust не менялись. Это silent fixture, а не Linux audio parity. Full — Engine=true, Team — Engine=false/source_root=null; Full/Team `--check` и Team→Player работают.

X11 и Wayland — complete=true/measurements_valid=true, по90 stopped/play frames, verified Play/Stop, errors=[]. GUI и game device — Vulkan / llvmpipe (LLVM20.1.2), то есть software Vulkan в WSLg. X11 handler p95=8.56/11.04ms, callback interval p95=23.75/28.97ms; Wayland handler p95=9.27/10.77ms, callback interval p95=60.81/69.81ms. Это функциональная native acceptance, а не достижение60FPS. X11 сообщает focused/visible=90/90 samples; Wayland focused=90, visibility unknown. Game-worker diagnostic passed. Hardware Linux/другая glibc/дистрибутив этими данными не сертифицированы.

## Что считается завершением

Каждый P2 должен пройти содержательные regression tests и актуальные Windows/Linux интеграционные проверки. Отдельно проверяется Stop/reuse: unchanged-context root не пересобирается, state и worker PID независимы. Вызов Cargo/freshness proof не считается root compilation; Workbench acceptance не считается новым desktop GUI/physical presentation benchmark.

Команды и exit codes записаны в перечисленных logs/summary, source/branch/dirty snapshot и protected SHA — в `stage0-baseline.json`, `stage6-source-snapshot.json` и `final-verification.json`. Старые585/594 test counts и оконные замеры не считаются приёмкой нового source. Строгий Player60FPS ранее failed и остаётся незакрытым без нового соответствующего замера. Linux WSLg/llvmpipe, native audio и dialogs обозначаются явно; аппаратная Linux-сертификация в этот этап не входит.

## Сборка, исходники и оставшиеся ограничения

Проверенная новая Windows сборка: `outputs/ModuleEngine-review-fixed.exe`, SHA256 `A5E4F9E85E632AF7E6CA111A040A3DF78282B12684FE1C0C671F41E2CCCA5FBC`, code commit `566eafb`. Исходный `outputs/ModuleEngine.exe` остаётся SHA `2477792D79F64A360AE7BEE34DF1BA81DEBEFB949FE6A4E6FA19EF79C5E66BD3`; его recoverable копия — `prior-ModuleEngine-17fb340.exe` в evidence. Для возврата к прежнему опубликованному редактору достаточно запускать сохранённый исходный EXE; прежняя ветка/commit17fb340 тоже сохранены. Новая сборка и source inventory не требуют перезаписывать авторские проекты.

Дополнительная Windows relocation: один созданный здесь Player перенесён в `windows-player-relocated`, SHA сохранён, повторный compiler-less verify exit0; provenance переноса — `stage6-windows-relocation.json`. Три первоначальные package paths в раннем Windows manifest относятся к моменту до переноса.

Четыре исходных P2 исправлены в заявленном объёме; это не заявление, что весь движок или все будущие случаи свободны от ошибок. Открыты PropertySchema default-only asset dependencies, отдельный P3 выбранной composition в Assets browser, strict Player60FPS, cold Editor preparation within45s, native Linux audio/dialogs и hardware Linux/foreground input-to-display QA. Полное визуальное редактирование Rust, другие render API и прочие большие подсистемы этим проходом не добавлялись.

Все source изменения выполнены в одном Git checkout, в ветке `codex/postfix-p2-review-fixes`. Full/Team/Player папки — необходимые export acceptance fixtures, а не новые development copies. Dependency targets разделяются между сборками; code/ABI/frontend/backend profiles, benchmark/watchdog/IPC limits, ОС и драйверы не менялись. OS background-процессы других задач не завершались. После финальной сверки protected данные совпадают с baseline, а tracked tree чистое.
