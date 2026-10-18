# Продолжение P0: доказательства показа кадров на GPU — 18 октября 2026

[English](../p0-continuation-20261018.md) | [Русский](p0-continuation-20261018.md) | [Документация](README.md)

**Первая допустимая холодная попытка на восстановленной обычной Windows-сборке и текущие ограниченные маршруты Windows ARPG, Windows/Linux offscreen и Windows authoring пройдены. Обычное закрытие чистого Linux-окна и synthetic release-failure pair также пройдены в указанных ниже границах; прежние неудачные квитанции fixtures сохранены. Это не полная приёмка P0.** Продолжение обновляет доказательства после [исходного GPU-отчёта](gpu-presentation-20261018.md); его неудачные прогоны и исторический [P0 от 17 октября](p0-stabilization-20261017.md) сохраняют прежний статус.

## Привязка исходников и артефактов

HEAD: `cbe97de7a974d08e391b4e3b21a18656d08ff6f7`. Обычные Windows-исходники: `cedf173e70f531c8f1784a14ec02830a4aea905f7ff0ebd3aea5a0a07484a6cb`; [inventory](../../outputs/verification/p0-continuation-20261018/normal-source-01.json), [квитанция Editor](../../outputs/verification/p0-continuation-20261018/normal-editor-receipt-01.json). SHA256 обычного release Editor: `d061c7f69b0b1df38b6564875e9a92c4b1deec56229c5c7f9a83a0ef7932548e`.

[Квитанция восстановления](../../outputs/verification/p0-continuation-20261018/probe-restoration-01.json) фиксирует точное восстановление исходного runtime и удаление временного диагностического модуля. Production admission default остаётся 2; оптимизация планирования или исправление таймаута не сохранены. После восстановления изменён только adapter test example: проверяется отказ production adapter и сохранность при неверном Windows LUID или Linux device/driver UUID. Остальные native inputs source15 неизменны. Согласование документации ME-01 имеет [отдельную квитанцию](../../outputs/verification/p0-continuation-20261018/me01-receipt.json).

Linux inventory `6cb94aa78ff59df58ce2ee234169d6094b26799e97deb229c1293edff3453f10` содержит те же 162 native inputs и только дополнительную постороннюю ignored-конфигурацию. [Привязка inventories](../../outputs/verification/p0-continuation-20261018/linux-normal-inventory-binding-01.json) отличает различие политики учёта от изменения продукта. Linux остаётся Ubuntu/WSL llvmpipe software Vulkan, без аппаратной сертификации Linux.

## Первая допустимая обычная target-cold попытка

[Cold normal01](../../outputs/verification/p0-continuation-20261018/cold-normal-01/launch.json) проходит неизменённые process-wall и internal отсчёты ≤45 с: **40,906 / 39,9128549 с**. Это первая допустимая обычная попытка после восстановления: новый процесс, свежий фактический Full export и ранее отсутствовавший `work/ctN01`, существующие tools/offline registry, неконтролируемые кэши ОС. Это не прогретая подмена и не гарантированная граница времени запуска.

[Native report](../../outputs/verification/p0-continuation-20261018/cold-normal-01/report.json) завершает 90+90 измеренных кадров и 30+30 разогревов, фактические graph pans и готовность Graph/Scene/Game. Play/Stop освобождает прежнего владельца, создаёт actor 2 после actor 1 и достигает stopped readiness на time zero. Оба отсчёта включают эту итоговую готовность. Source/protected/Full checks проходят, сохраняя все 33 защищённых хэша; [независимая cold-проверка](../../outputs/verification/p0-continuation-20261018/cold-normal-01-independent-check.json) подтверждает конкретный случай. Инъецированные движения указателя и dispatch команд — scripted native UI evidence, без сертификации физического ввода или scanout.

Диагностическая основа cold-trace01 прошла 41,343/40,3415648 с; временный контроль limit 1 — 40,313/39,3508023 с. Это диагностические сборки, не shipping acceptance. Контроль убирает лишнюю начальную попытку подготовки, но одна пара при разных временах compilation/cache/scheduling не позволяет приписать admission весь wall delta. Итоговый Stop всё ещё выполняет независимую тёплую подготовку. Прежние cold01 os5 и final-Stop watchdog failures остаются неудачами; их причина неизвестна. Работа локализует успешную подготовку и фиксирует успешную обычную попытку, но не доказывает исправление или GPU causation.

## Корректность транспорта GPU

[Windows fixture](../../outputs/verification/p0-continuation-20261018/windows-gpu-fixture-01/result.json) проходит 22 сценария/16 точных сравнений изображений на RTX 4080 SUPER/DX12. [Linux fixture](../../outputs/verification/p0-continuation-20261018/linux-gpu-fixture-01/result.json) проходит 23/16 на WSL llvmpipe/Vulkan, включая раздельные отказы при неверном device UUID и driver UUID. Обычные CPU readback/upload равны нулю; явные чтения для сравнения диагностические. Windows fixture SHA256: `0c879d73721ea5af4884b43c3f33d6f0beb1adb2e706bd3298895ff7c712c211`; Linux ELF: `7554cf26fa2dc384257ae98ea7324d782935c5f9b6d0e97a179c1a6eef704c2e`.

Production-bridge проверки offscreen: без surface, eframe paint, vsync или compositor. Они не сертифицируют оконную частоту, физический scanout, произвольные drivers/ABI или исключительный Linux ownership-release failure. Текущий обычный Linux Editor также успешно собран; сборка не является приёмкой окна.

## Текущие Windows ARPG и самостоятельная поставка

[Текущий ARPG relocation](../../outputs/verification/p0-continuation-20261018/arpg-relocation-01/result.json) проходит все пять фактических команд: normal Editor benchmark, поддержанный selected export, проверку перенесённого пакета, headless check и standalone native benchmark. Неизменённый критерий 1080p/20 живых врагов/Playing пройден: Editor **119,05020347 FPS / worker p95 9,4335 мс**, Player **119,002023 FPS / worker p95 9,2239 мс**, без frame/wait errors. Это отдельные серии presentation и worker; симуляция остаётся фиксированной на 60 Гц.

Перенесённый Player из 18 файлов имеет точные features `arpg,audio,ui,vulkan`, SHA256 `9887f1587da193eb9eca5dce935239e08f588b177a6435b8a4239b3cbc8eda70` и manifest SHA256 `0649cd88b458a73bb58bfdf8aa75dcae2fc524fd933b57ee43702f6353fa2807`. Tools-free запуск и сохранность проходят. Этот source-bound прогон — не новое A/B-распределение, универсальное ускорение или clean-OS certification; прежние source15 A/B доказательства остаются отдельно привязанными.

## Ограниченный Windows authoring маршрут

[Квитанция UI-действий](../../outputs/verification/p0-continuation-20261018/authoring-ui-01/ui-actions.json) и четыре фактических JPEG фиксируют New Empty → add object → Scene X-gizmo drag → Save → native Open → Play/Stop → Windows Player export. Editor нормально завершается с кодом 0 при сохранённых source/protected/Full/EXE. Сохранённый object ID1 имеет translation `[3.102818,0,0]`; authored SHA256: `27acc01e0943e717102425cd3d11875f71f6f22b48a4abd6b41f67997165151a`.

[Перенесённый authored Player](../../outputs/verification/p0-continuation-20261018/authoring-player-02/result.json) проходит фактические `--verify-package` и `--benchmark`, 120 разных кадров, продвижение симуляции и отсутствие frame/wait errors без инструментов в окружении запуска. Пакет `ui,vulkan` из 10 файлов имеет Player SHA256 `77aad97859f90145d6a484431ae86676e5ff4a5c99b2ac9b3022676fdd5a642c`. Полное сравнение с учётом JSON-типов доказывает семантическое равенство при различии байтов сериализации. Квитанция переписана автором из agent Computer Use; независимый reviewer проверил captures/artifacts, не повторяя каждый ввод. Numeric text editing и первоначальные tool input-geometry misses не проверены, не засчитаны молча и не объявлены дефектами продукта. Ранние fixtures с неподдержанным checker flag и byte equality остаются неудачами fixtures.

## Linux window маршрут и оставшиеся доказательства

[Window01](../../outputs/verification/p0-continuation-20261018/me04-linux-window-01/result.json) проходит свежие Full export/capabilities, audio-free Puzzle и фактический resized Editor benchmark. Его plain WM_DELETE и последующий capture достигают таймаута, поскольку generated-проект показывает **Unsaved Changes** migration guard. Фактический capture устанавливает наличие modal; анализ исходников объясняет принятие default asset metadata из `asset_manifest=null`. Это не доказательство GPU hang. Cleanup receipt capture v3 остаётся неподтверждённым несмотря на позднее отсутствие PID; последующий успех не меняет его статус.

[Чистый plain-close случай](../../outputs/verification/p0-continuation-20261018/me04-clean-plain-close-01/result.json) меняет только `asset_manifest` собственного generated fixture с null на canonical default, сохраняя оригинал, одинаковый binding и отсутствие assets directory. Фактический текущий Editor получает WM_DELETE и завершается0 за **5,401733667 с**, окно исчезает, собственная process group отсутствует. Все source/protected/Full/Puzzle/clean-fixture/ELF/helper проверки проходят; retirement counters0/0/0/0/0. [Независимое window review](../../outputs/reviews/p0-continuation-20261018/me04-window-review.md) принимает этот случай. Один записанный X11 BadWindow (error3/request3) относится к post-exit запросу сохранённого window ID; нулевые X11 errors не заявляются. Это функциональный X11 window-close случай без clicks или Computer Use, не GPU workload, hardware/Wayland/cold/performance acceptance и не обход unsaved-work guard.

Native Linux WAV/dialog services, hardware/ABI portability и physical foreground/scanout certification остаются вне этих доказательств. Успешные formatting и selected-example Clippy не заменяют runtime acceptance. [Независимое ограниченное review](../../outputs/reviews/p0-continuation-20261018/newnormal-review.md) отличает собственные проверки от выполнений автора. Итоговые continuation review/source binding следуют за завершёнными доказательствами и заморозкой документации; полный workspace rerun и полное закрытие P0 не заявляются.

## Linux synthetic release-failure pair — фактический PASS

[Operation06](../../outputs/verification/p0-continuation-20261018/linux-fault-operation-06.json) и [pair06](../../outputs/verification/p0-continuation-20261018/linux-fault-pair-06/result.json) проходят на отдельно привязанных Linux-исходниках6cb. Драйвер fixture вызывает фактический production bridge; явный Vulkan layer перехватывает соответствующую release submission и возвращает pre-submit `ERROR_DEVICE_LOST` (-4), не передавая её драйверу Vulkan. Каждый normal/fault прогон имеет ровно один matching release submit и `invalid=false`. Оба фактических процесса завершаются0 и подтверждают отсутствие своих process groups. Это synthetic API-failure эксперимент на WSL llvmpipe, не реальная аппаратная потеря устройства.

| Наблюдение | Normal | Synthetic fault |
| --- | --- | --- |
| Registered/joined/completed/unconfirmed/failed retirement |1/1/1/0/0|1/1/0/1/1|
| Weak lease после join/owner drop |dead|live|
| Доступные слоты после освобождения остальных кадров |4|3|
| Наблюдённые target destroys |5|0|
| Обычные CPU readback/upload и diagnostic readback bytes |0/0/0|0/0/0|

Fault driver намеренно наблюдает production retirement `ok=false` и `GPU presentation ownership release failed; source lease remains pinned`; fixture PASS означает проверку ожидаемых fail-safe/error/quarantine. Это не означает успешный render, исправленную ошибку или сертификацию реальной потери устройства. Проверяется initial four-slot/backpressure; fault сохраняет затронутые lease/device после normal owner drop вместо повторного использования слота. Source/protected/control after-checks проходят. [Независимое actual-fault review](../../outputs/reviews/p0-continuation-20261018/me04-fault-review.md) принимает этот ограниченный эксперимент; итоговое решение фиксируется отдельно. Ранние header/dependency-pin/offline-metadata setup failures сохраняются как fixture/environment failures; итоговая пара не меняет их статус.

Приведённое выше чистое обычное закрытие завершает этот ограниченный функциональный маршрут. Pure leader-Z ownership evidence shared-v4 отдельно проверяет границу helper. Прежние header/dependency-pin/offline-metadata setup failures остаются fixture/environment failures с исходными квитанциями.

## Итоговая привязка и review

[Итоговый внешний source inventory](../../outputs/verification/p0-continuation-20261018/final-source-02.json) и [независимое итоговое решение](../../outputs/reviews/p0-continuation-20261018/final-review.md) привязывают завершённый снимок документации. При написании итоговое review ожидается; окончательный вывод дают эти записи. Native-результаты сохраняют исходные build/source identities; последующая документация не меняет привязку бинарников и не является полным workspace rerun. Полная P0, hardware Linux, real-device-loss, универсальная производительность или physical scanout certification не заявляются.
