# Производительность сборки и проверок

[English](../build-verification-performance.md) | [Русский](build-verification-performance.md) | [Документация](README.md)

Этот контракт разделяет компиляцию/линковку, прямое исполнение тестов, работу launcher
и перенос hosted cache. Быстрое локальное переиспользование не доказывает выигрыш
hosted CI. Объём тестов, выбор features/targets и проверка артефактов сохраняются.

## Метод замера и локальные доказательства

[Парный отчёт](../../outputs/ci-editor-builds-20261017/measurements/20261017T022121Z-8eb11023/result.json)
и его доказательства в `cold/`, `repeat/`, архиве `instrumentation/` описывают Windows
`contracts` на HEAD `0d3eb5285bc9674b2edb50e7830c8a69f7053d23`, source digest
`fad4e5438f83cbcc5cf0847b66cabb41d25f681532702ac0a16b2840d145c06a`.
Оба цикла прошли 187 Rust tests, сохранили все 33 защищённых файла и исходники.
Новый target directory `.work/b/8eb11023` не существовал до первого цикла;
второй его переиспользовал. Существующие offline dependency и OS/filesystem caches
были прогретыми/неконтролируемыми. Это target-cold против repeat, не machine-cold.

Оба цикла использовали обычный план `contracts`, `--offline --locked`,
`CARGO_INCREMENTAL=0` и новый дочерний TEMP каждого запуска. Compiler flags,
wrappers, profiles и тесты не менялись; общие caches не очищались.

| Wall time, секунды | Target-cold | Repeat |
| --- | ---: | ---: |
| Полный цикл | 182.375 | 89.906 |
| Команды Cargo compile + link | 92.376 | 3.905 |
| Прямое исполнение тестов | 81.641 | 81.250 |
| Test discovery | 2.296 | 0.499 |
| Formatting | 1.859 | 1.859 |
| Setup commands | 0.187 | 0.187 |
| Вне команд | 4.016 | 2.206 |

Время вне команд включает source/protected hashing, lease/setup, завершение observer
и отчёты; это не чистый overhead Cargo. Повторное исполнение тестов осталось близким
к 81 секунде даже при резком сокращении компиляции. Это обосновывает исследование
reuse compiled products, но не удаление тестов или изменение profiles. Зависимости
sccache или nextest не добавлялись.

Read-only Windows observer на Toolhelp/GetProcessTimes опрашивал принадлежащие
операции rustc/linker процессы каждые 50 мс. В холодной compile/link фазе approximate
exclusive compiler union составил 79.199 секунды, observed linker union — 10.343;
в repeat compile/link фазе compiler/linker не наблюдались. Опрос может пропустить
короткие процессы или ancestry, поэтому это оценки, не точные compiler timings.
CPU totals параллельных процессов отличаются от wall time. Repeat setup записал
одну OpenProcess race error; compile/link observation не имели ошибок или достижения
sample cap. Probe/source hashes и архив исходников observer находятся в отчёте.
Эти замеры не оценивают cold-start Editor, hardware Linux или hosted acceptance.

## Контракт hosted Cargo cache

[scripts/ci_cache.py](../../scripts/ci_cache.py) вызывается как
`python scripts/ci_cache.py` в существующем [CI checks job](ci.md). Он выводит JSON
решение о допуске и, если задан `GITHUB_OUTPUT`, публикует `enabled`, `key`,
`restore_prefix` и `native_host`. Он не меняет environment variables и не исправляет
неподдерживаемый контекст. Неизвестные flags/wrappers, действующие Cargo configs,
непроверенная toolchain/runner identity или другое неучтённое compiler environment
отключают caching; обычные проверки продолжаются с исходным окружением.

Для известного setup-python context предусмотрено узкое исключение из отказа по
loader/pkg-config. `pythonLocation` должен быть canonical каталогом
`$RUNNER_TOOL_CACHE/Python/<actual-3.12.patch>/x64`. Работающий interpreter должен
находиться непосредственно в нём на Windows или в его `bin` на Linux, а resolved
binary — внутри этой установки. Linux `LD_LIBRARY_PATH` допустим отсутствующим/пустым
или ровно `lib` этой установки; `PKG_CONFIG_PATH` — отсутствующим/пустым или ровно
её `lib/pkgconfig` (допускается эквивалентное Windows написание слешей). Дополнительные,
неизвестные или пустые элементы списка путей отклоняются. Другие непустые overrides
с `PKG_CONFIG` и preload/audit overrides отключают cache; окружение сохраняется.

Compatibility hash включает настоящий interpreter binary и ограниченные native
library/pkg-config inputs. Linux хеширует непосредственные файлы `lib` и referents
symlinks внутри установки, а также shallow names/types; допустимы только подкаталоги
`python3.12` и `pkgconfig`, требуется `libpython3.12.so*`. Windows хеширует DLL
непосредственно в installation directory, требует `python312.dll`, а также учитывает
непосредственные loadable libraries в `lib`, если они есть; standard library `Lib`
рекурсивно не хешируется. Непосредственные `.pc` файлы хешируются; вложенные или
неизвестные pkg-config inputs, нечитаемые файлы и выходящие наружу links отклоняются.
Поверхностный просмотр Windows `Lib` допускает 512 entries; обычные `.py` файлы и
подкаталоги standard library рекурсивно не хешируются, а подходящие непосредственные
native libraries хешируются по содержимому. Остальные просмотры каталогов допускают
128 entries. Общий предел учитываемых файлов остаётся 128 файлов и 256 МиБ.
Отсутствующие Windows library/pkg-config directories представлены явно.
Это ограниченный inventory, а не полная system-library closure; compatibility key
также привязан к runner image.

Независимое ревью R3-1 обнаружило ранее неучтённые pkg-config overrides; первый Linux
trial также отключил caching из-за известного loader path setup-python. Исправленная
policy допускает только точный проверенный контекст выше. Ни такой допуск, ни локальный
repeat не доказывают Linux cache hit или чистый hosted выигрыш; реальные CI результаты
остаются в отчётах своих прогонов.
Workflow использует один объединённый restore/save action `actions/cache` v6,
закреплённый на `55cc8345863c7cc4c66a329aec7e433d2d1c52a9`. Допустимы только пути:

- `$CARGO_HOME/registry` и `$CARGO_HOME/git`;
- `target/debug` и `target/release`;
- `target/<verified-native-host>/debug` и `target/<verified-native-host>/release`.

Custom worker и `sdk-v3` namespaces, авторские/root данные, сгенерированные fixtures,
`.tools`, отчёты и credentials не входят в этот cache scope. Это не резервная копия
SDK, Custom-program-store или пользовательского проекта. Изменение списка требует
повторной проверки этих границ.

Compatibility prefix хеширует проверенные verbose Rust/Cargo identity, native host,
runner OS/architecture/image и пути, toolchain pins, lockfile, manifests и profiles,
CI/verification policy и защищённые configuration inputs. Скрипт также включает
консервативные source-policy inputs, в том числе исходники Core/runtime-script.
Exact key дополняется source SHA. Restore prefix только один: полный compatibility
prefix без более широкого fallback между несовместимыми контекстами. Miss или отказ
cache admission означает обычный прогон без cache.

После restore по-прежнему выполняются каждый locked dependency fetch, Clippy,
contracts, выбранный native/package маршрут и artifact check. Hit не пропускает
проверки и не наследует прежний pass. Существующие Cargo freshness и artifact-selection
проверки по-прежнему определяют допустимость переиспользования compiled products.

## Оценка hosted выигрыша

Сравнивайте hosted прогоны с одинаковыми routes, toolchain, runner image и значимыми
исходниками; различайте uncached baseline, совместимый miss и совместимый hit.
Записывайте cache decisions, размер/время переноса, restore и save, компиляцию,
тесты и полное время job. Учитывайте post-job save и отказы, а не только строку
времени компиляции Cargo. Сохраняйте исходные GitHub logs и verification artifacts
текущего прогона вместе с каждым отчётом.

Один локальный repeat не доказывает hosted выигрыш; фактические результаты прогонов
относятся к своим отчётам. Uncached baseline —
[PR #6 run 38994338494](https://github.com/myjname/ModuleEngine/actions/runs/38994338494).
Это руководство не заявляет ускорение cache относительно baseline и не предсказывает
результат miss/hit. Перенос cache может стоить больше предотвращённой работы.
Caching не меняет fail-closed aggregate `CI required` или границы полной приёмки.
