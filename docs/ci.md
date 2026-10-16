# CI ModuleEngine

Цель: каждый PR получает `contracts` на Windows и Linux; изменение исполняемых
исходников добавляет native и package проверки. Документационный PR не получает
дорогие native/package сборки. Нецели: performance, desktop dialogs, hardware GPU
и полная Full/Team/Player поставка. Критерий: `CI required` успешен только после
успеха планирования и обеих платформ, включая выбранные дополнительные маршруты.

## Что выполняется

[Workflow](../.github/workflows/ci.yml) запускается для всех PR, push в `main`, merge queue
и вручную; workflow-level path filters отсутствуют. [ci_plan.py](../scripts/ci_plan.py)
читает Git diff, включая удалённые файлы и обе стороны rename. Только изменения
в `docs/`, Markdown, LICENSE и `.gitignore` пропускают дополнительные маршруты;
неизвестный новый source path включает их. Initial push/manual run включает всё.
Ошибка определения diff завершает планирование ошибкой.

| Runner | Всегда | При изменении исходников |
| --- | --- | --- |
| `windows-2022` | Python unit checks, `contracts` | `native-tools`, затем `package` |
| `ubuntu-24.04` | Python unit checks, `contracts` | `native` на явно выбранном Mesa lavapipe, затем `package` |

`native-tools` — три существующие проверки native linker/cache/executable
spellings без GPU или UI. Это часть `native`, а не замена его двух GameUI tests.
Linux full `native` запускает offscreen tests с software Vulkan, без desktop
сессии. Windows GPU native acceptance остаётся адресным локальным маршрутом.
Успешный CI не подтверждает hardware Linux, строгие 60 FPS, cold-start или
интерактивную desktop приёмку. `package` использует минимальные Cargo fixtures,
не настоящий game export.

Две платформы выполняются параллельно на отдельных hosted VM; команды внутри
каждой VM выполняются последовательно. Каждый job получает новый hosted checkout
и отдельные caches/TEMP; авторский рабочий каталог разработчика не используется.
Rust version/native target берутся из существующих Windows/Linux manifests.
Перед offline launcher CI явно устанавливает pinned Rust/rustfmt и делает
`cargo fetch --locked`. Linux явно устанавливает системные link библиотеки;
native маршрут дополнительно устанавливает Mesa и восстанавливает DXC по
существующему archive SHA и всем file SHA из `scripts/linux-toolchain.json`.
Сбой установки, fetch, компиляции или теста — отказ CI, без fallback версии.

Launcher использует короткий checkout-local `.work/v/<run-token>` и записывает
его resolved путь в отчёт. Первый Windows hosted run обнаружил LNK1181 в nested
snapshot fixture на длинном TEMP; тест и его критерии сохранены. Точное имя
проблемного object скрыто Cargo, поэтому причина path length — проверяемая гипотеза,
а подтверждение исправления требует нового успешного `package` на hosted Windows.

Отчёты и command logs из `outputs/verification` публикуются как artifact для
каждой платформы даже после отказа; если проверка не стартовала, GitHub job log
содержит ошибку подготовки. Retention 14 дней. Full source/artifact hashes и
scope остаются в обычном `result.json` launcher. CI caches пока не добавлены:
сначала нужно измерить реальный hosted прогон и не перенести локальные tool paths.

## Включение обязательности

Файл workflow сам по себе не меняет GitHub branch protection. После появления
workflow в целевом GitHub repository и первого успешного PR/push запуска:

1. В ruleset/protected branch включить PR перед слиянием.
2. Добавить required status check **`CI required`**, source GitHub Actions.
3. Проверить отказ теста в отдельном PR: failed prerequisite обязан блокировать
   `CI required`. Новая версия PR должна пройти повторно; старый SHA не подходит.

Aggregate с `if: always()` явно отвергает `failure`, `cancelled`, `skipped` и
отсутствующие route outputs. Отдельные условные native/package steps не объявляются
пройденными, если их маршрут не выбран. Если branch protection/ruleset ещё не
применён, CI даёт сигнал, но технически не запрещает merge. Включение правила
требует известного repository и соответствующих прав; локальные unit checks
не доказывают выполнение GitHub Actions.

Токен workflow имеет только `contents: read`; checkout не сохраняет credentials.
Нет secrets, `pull_request_target`, привилегированного PR исполнения или
неподготовленного self-hosted job. Все внешние actions закреплены full commit SHA.
Загрузка fresh CI tools выполняется только внутри явно запущенного hosted job.

Для [myjname/ModuleEngine](https://github.com/myjname/ModuleEngine) включён public
доступ по решению владельца. На `main` применены required PR, актуальная ветка,
**`CI required` с GitHub Actions app ID 15368**, защита администратора и завершение
review-обсуждений; force push и удаление ветки запрещены. Required human approvals = 0:
отдельное независимое AI-ревью не выдаётся за approval другого GitHub-аккаунта.
В [служебном PR #2](https://github.com/myjname/ModuleEngine/pull/2) неверный base SHA
дал failure планирования, skipped matrix и failed aggregate; GitHub показал
`mergeable_state=blocked`. PR закрыт без слияния. Это доказательство отказа
prerequisite, а не отдельный искусственный отказ Rust-теста. Реальные результаты
каждой версии нужно смотреть в её [CI run](https://github.com/myjname/ModuleEngine/actions).

## Локальная проверка политики

```sh
python -B -m unittest discover -s scripts -p 'test_*.py' -v
python scripts/ci_plan.py --base <full-base-SHA>
python scripts/verify.py --profile native-tools --plan
```

Policy fixtures проверяют выбор путей, безопасный Git argv и отказ aggregate на
ошибке/skip/cancel. Это не hosted CI acceptance. Изменённый workflow нужно проверить
в GitHub, включая fresh dependency restore на обеих системах.

Основания: [required checks и skipped jobs](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks),
[минимальные permissions и SHA pins](https://docs.github.com/en/actions/reference/security/secure-use),
[hosted runner images](https://github.com/actions/runner-images).
