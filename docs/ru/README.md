# Документация ModuleEngine

[English](../README.md) | [Русский](README.md)

Основной язык документации — английский. Для каждого документа ниже есть полная русская версия, доступная по ссылкам переключения языка. Начните с текущего состояния; исторические отчёты подтверждают только указанные в них снимки и проверки.

## С чего начать

- [Работа с ассистентом ModuleEngine](assistant-workflow.md)
- [Диагностика поведения ModuleEngine](debugging-skill.md)

- [ModuleEngine 0.4](../../README.ru.md)
- [Работа над ModuleEngine](agent-guide.md)
- [Текущее состояние ModuleEngine](current-status.md)
- [Архитектура 0.4](architecture.md)
- [Решения и рабочие допущения](decisions.md)
- [Открытые вопросы после 0.4](questions.md)
- [Игровой этап: от авторской сцены до Player](next-stage.md)

## Редактор и авторские данные

- [Меню редактора и работа с проектом](editor-workflow.md)
- [Авторские сцены и граф игровых систем](editor-game.md)
- [Scene и Assets](scene-assets.md)
- [Авторские сцены, шаблоны и Play-копии](scenes.md)
- [Настройки Engine и Project](settings.md)
- [История, объединение и ресурсы](history-assets.md)
- [Ассеты: идентичность, ссылки и файловые операции](asset-workspace-api.md)
- [Схемы компонентов из исходников и композиция](component-schemas.md)
- [Визуальная обратная связь ближнего боя](combat-feedback.md)

## Runtime, модули и рендеринг

- [ECS-хранилище игрового мира](ecs-world.md)
- [Решение о хранилище игрового мира](ecs-storage-decision.md)
- [Исполняемые системы и авторское расписание](module-execution.md)
- [Формат проекта 0.4](graph-format.md)
- [Переиспользуемые игровые модули и приёмочная игра](gameplay.md)
- [Опциональные игровые пакеты](optional-gameplay.md)
- [Игра-головоломка с ящиком](puzzle-project.md)
- [Общий хост авторской игры](shared-game-host.md)
- [Исполнение Game вне UI](game-worker.md)
- [Custom Rust 0.4](custom-rust.md)
- [Состояние Rust Custom и самостоятельные workers](custom-state.md)
- [Повторное использование Custom Rust](custom-program-store.md)
- [Статические модели, текстуры и материалы](content.md)
- [Vulkan и Custom HLSL](gpu-hlsl.md)
- [Добавление модуля](extending.md)

## Сборка, поставки и проверки

- [Закреплённые инструменты Windows](toolchain.md)
- [CI ModuleEngine](ci.md)
- [Маршруты проверок](verification.md)
- [Независимое ревью изменений](review.md)
- [Review: <задача>](review-template.md)
- [Сквозная приёмка игры и доказательства замеров](game-verification.md)
- [Профили Engine, совместимость и самостоятельный Player](engine-profiles.md)
- [Full, Team и отдельный Player](distribution.md)
- [Самостоятельная игра и проверки](player.md)
- [Подготовка Linux и границы проверки](linux-support.md)
- [Обслуживание воспроизводимых артефактов](artifact-maintenance.md)
- [Опциональные сервисы хоста](../../crates/host-services/README.ru.md)
- [Самостоятельный Player](../../crates/player/README.ru.md)

## Отчёты этапов и исторические свидетельства

- [Результат игрового этапа 0.4](release-v04.md)
- [Этап UI: меню и работа с проектом](release-ui.md)
- [Scene, Assets и отзывчивое редактирование](release-scene-assets.md)
- [Функциональность и стабильность 0.4 — 15 октября 2026](stability-audit.md)
- [Миграция модульного runtime](modular-runtime-progress.md)
- [Подготовка Linux: проверка 15.10.2026](linux-support-20261015.md)
- [Зависания рабочего стола при запуске ModuleEngine — 15.10.2026](desktop-stalls-20261015.md)
- [Исправления ревью и повторной подготовки Custom — 16.10.2026](review-fixes-20261016.md)
- [Четыре дополнительных P2 после ревью 17fb340](postfix-p2-fixes-20261016.md)
- [PropertySchema assets и сохранность Inspector после ревью 5e3f064](remaining-authoring-fixes-20261016.md)
- [Пути системных диалогов и транзакция Save As](dialog-path-fixes-20261016.md)
