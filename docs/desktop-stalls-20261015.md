# Зависания рабочего стола при запуске ModuleEngine — 15.10.2026

Причина локализована в пути показа окна OpenGL/Glow: поток композиции Windows DWM тратит почти целое ядро на поиск события DX BLT внутри `OpenDxBltEvent`. Изменение GUI на WGPU/DX12 устраняет воспроизведение секундных остановок обновления основного рабочего стола в контролируемом сравнении. Игровые ECS, Host и Vulkan не менялись.

Ветка: `codex/fix-dwm-desktop-stalls`; исходный HEAD: `4cee54d40469afd9d59bb101d67af0a2176e5ece`. Commit реализации: `c714e9e7521d3203c86dd2b9c7e0574099cfa2bf`; отчёт и source inventory сохранены следующим commit. Компактные числовые доказательства: [desktop-stalls-20261015.json](evidence/desktop-stalls-20261015.json).

## Причина и границы установленного

Исходная ETW `E:\ModuleEngine\outputs\independent-audit\lag-diagnostics\trace-admin-20261015-102351\engine-lag.etl` сохранена: 543162368 байт, SHA-256 `E407947D4705EDBD72E491DB84CB6AD27A67C2FAA67EE2666B02419CD41A9D01`. Потерь событий/буферов нет. Сохранено предупреждение xperf об одном `InvalidEvent` на 22,607578 с в rundown другого процесса; массовой потери данных оно не означает.

DWM PID 2208, TID 3004 завершал `SCHEDULE_PRESENT` за максимум 2,823 мс до запуска. После запуска зарегистрированы 341,434; 2018,662 и 8123,493 мс. Последний интервал остался незавершённым до конца трассы. Аппаратный VSync продолжался примерно с 144 Гц; отправка движком Present не означает фактический показ кадров.

Публичные PDB Microsoft точно сопоставлены с GUID/age из ETL. Стек:

```text
NtQueryDirectoryObject → OpenDxBltEvent
→ CGlobalSurfaceManager::AdvanceFrame → CComposition::PostRender
→ CComposition::ProcessComposition
```

За 11 секунд TID 3004 получил 10,999 секунды CPU; 10727 из 10995 CPU samples (97,56%) проходят через `OpenDxBltEvent`. Только `NtQueryDirectoryObject` занял 8,706 секунды, поиск имени объекта — ещё 1,120 секунды. Расшифровка соответствующего системного DLL показывает: после неудачного `OpenEventW("DwmDxBltEvent_…")` DWM перебирает `\Sessions\<session>\AppContainerNamedObjects`, пытаясь открыть событие в каждом подкаталоге.

**Почему первоначальный `OpenEventW` неуспешен, не установлено.** Отсутствие события, ошибка доступа и конкретная ошибка Windows/драйвера не различаются имеющимися данными. Позднее чтение каталога обнаружило 2506 объектов; их происхождение и причинная роль не доказаны. Объекты не менялись.

Пустое окно eframe без проекта, GameHost и Vulkan воспроизвело проблему: средняя загрузка DWM — 94,67% одного ядра с Glow и 8,05% с DX12, без Desktop Duplication. Поэтому игровой рендеринг, ECS и стартовая компиляция не нужны для воспроизведения. Переполнение RAM/VRAM, постоянная общая загрузка CPU/GPU и секундные DPC/ISR не подтвердились. Причинность MPO и сторонних overlay не установлена. В исходной ETW горячий DWM сохраняется после закрытия; немедленное восстановление этой трассой не доказано.

## Изменение и сравнение

`E:\ModuleEngine\crates\player\src\ui.rs` задаёт общую политику Editor/Player: Windows GUI по умолчанию использует WGPU, с явным выбором DX12. `MODULE_ENGINE_GUI_BACKEND=glow` оставлен для диагностики. Инициализированный адаптер записывается отдельно от запрошенного backend. Подключение Editor: `E:\ModuleEngine\crates\editor\src\main.rs`; контрольное пустое окно: `E:\ModuleEngine\crates\editor\examples\desktop_presentation_probe.rs`. Зависимости обновлены в `Cargo.toml`/`Cargo.lock`.

Нативные снимки Editor поддерживает через `MODULE_ENGINE_UI_SCREENSHOT_TO` (`crates/editor/src/ui_capture.rs`), сохраняя WGPU. Старый `EFRAME_SCREENSHOT_TO` при неуказанном backend выбирает Glow; вместе с явно выбранным WGPU возвращает понятную ошибку. Для обычного запуска эти переменные должны отсутствовать.

A/B выполнялся последовательно одним Release EXE, с одинаковыми проектом, layout, настройками observer и основным монитором RTX 4080 SUPER, 3440×1440. Границы открытия определены QPC от resume до запроса закрытия. Desktop Duplication сохранял только метаданные, без чтения/копирования пикселей приложением observer.

| В интервале открытия | Glow, 10,001 с | WGPU/DX12, 10,014 с |
| --- | ---: | ---: |
| Максимальный завершённый разрыв обновлений | 2437,964 мс | 30,232 мс |
| Незавершённый интервал у последнего наблюдения | ≥6406,406 мс | ≥0,063 мс |
| Максимальный наблюдённый возраст изображения | 6406,406 мс | 28,367 мс |
| DWM, взвешенная загрузка одного ядра | 94,31% | 16,09% |
| Максимальный промежуток между опросами observer | 108,034 мс | 30,241 мс |

Разрыв Glow 8283,560 мс завершился на +11,801 с, уже после закрытия; он не включён в завершённые разрывы внутри открытия. Pointer-only события не сбрасывали возраст изображения. Продолжавшиеся опросы и отсутствие накопленных кадров в Glow не объясняют секундный провал остановкой observer.

Дополнительное обычное окно с Game view работало 45 секунд без Desktop Duplication; средний DWM — 12,91% ядра. Computer Use не получил подтверждение доступа вовремя: снимков и нажатий кнопок в этом прогоне не было. Это не проверка Play/Stop.

## Проверки, воспроизведение и оставшиеся ограничения

Прошли Release-сборка A/B, 3 теста `native_options_tests` и Clippy с `-D warnings`:

```powershell
$env:CARGO_HOME='E:\ModuleEngine\.work\cargo-home'
cargo build --release --offline --locked -p module-engine-editor --bin module-engine-editor --example desktop_presentation_probe
cargo test --release --offline --locked -p module-engine-player --features ui native_options_tests -- --test-threads=1
cargo clippy --release --offline --locked -p module-engine-editor -p module-engine-player --all-targets -- -D warnings
```

Повтор сравнения ограничен таймером; имена каталогов должны быть новыми:

```powershell
$diag='E:\ModuleEngine\outputs\independent-audit\lag-diagnostics\fix-investigation'
& "$diag\Run-DesktopCase.ps1" -CaseName repeat-glow-01 -Backend glow -ObserveDesktop
& "$diag\Run-DesktopCase.ps1" -CaseName repeat-dx12-01 -Backend wgpu -ObserveDesktop
& "$diag\Analyze-DesktopCases.ps1" -CaseDirectories @("$diag\repeat-glow-01","$diag\repeat-dx12-01") -OutputPath "$diag\repeat-analysis.json"
```

Полные локальные сводки: `$diag\offline-etw-localization.json`, `desktop-case-analysis.json`, `desktop-case-claims.json`; исходные CSV сохранены в каталогах случаев. Исходный проект и оба personal layout восстановлены побайтно: SHA-256 совпадают с началом диагностики; старый `game-project.json` также сохранён.

Финальная Release-сборка Editor и Player прошла с UI/ARPG/Puzzle/Vulkan/audio. Native benchmark в изолированной Full-копии прошёл 90 остановленных и 90 игровых кадров: симуляция продвинулась, команда Stop немедленно освободила Game owner, затем обычная подготовка Game-панели создала остановленный worker `actor_id=2` вместо `1`, время `0`. `play_stop.verified=true`, `measurements_valid=true`, ошибок нет; авторский документ и файл не изменились. Это проверка общего dispatcher toolbar, а не внешнего клика. За 17,87 с жизни процесса максимальный разрыв обновлений desktop — 35,586 мс, взвешенный DWM — 21,39% ядра. Источники: `$diag\native-editor\report.json` и `native-desktop-analysis.json`.

Итоговый `E:\ModuleEngine\outputs\ModuleEngine.exe` обновлён и совпадает с `target\release\module-engine-editor.exe`: SHA-256 `A40ED305456F9AFF3EE868CC91BCF86B34BDDB0111E35C3821C5DF1FD1F0069F`. Его обычный запуск на исходном проекте/layout без backend override, capture и Desktop Duplication прошёл 12 с: DWM в среднем 12,91% ядра, штатный exit 0, принудительного завершения не было, собственных дочерних процессов не осталось. Это дополнительная проверка CPU/cleanup; интервалы показа без observer здесь не измерялись. Источник: `$diag\final-exe-no-observer\summary.json`. Предыдущий EXE сохранён как `$diag\ModuleEngine.before.exe`.

Встроенный WGPU screenshot создан и просмотрен (`$diag\dx12-game.png`): он подтверждает захват GUI во время подготовки; готовые Game/Scene pixels подтверждены отдельно native benchmark. Source inventory обновлён штатной командой `--seal-source-project`; Full Engine доступен, `modified_sources=[]`, diagnostics пусты. Windows GUI теперь требует DX12 adapter. Существующие экспорты и prebuilt Player автоматически не обновляются; их файлы не менялись.

Новая ETW не записывалась: дополнительные права/UAC не получались. Desktop Duplication измеряет обновления изображения выбранного монитора и может менять режим презентации; это не длительности `SCHEDULE_PRESENT`, физическое сканирование дисплея или задержка ввода. Ограничения обычного запуска и проверки Play/Stop указаны отдельно. Драйверы, настройки безопасности, MPO, overlay и глобальные настройки Windows не менялись.
