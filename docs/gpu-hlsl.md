# Vulkan и Custom HLSL

Renderer исполняет raster и compute проходы на Vulkan. В редакторе HLSL компилируется DXC в SPIR-V; renderer создаёт pipelines, изображения и барьеры. При экспорте Player шейдеры компилируются заранее. Два встроенных пути различаются реальной последовательностью GPU-работы.

| Проход | Вход/результат |
| --- | --- |
| gpu.forward | Scene3D → HDR color, world normals, linear camera depth. |
| gpu.gbuffer | Scene3D → linear albedo, normals, depth. |
| gpu.deferred | G-buffer → HDR lighting. |
| gpu.depth_normal | Scene3D → normals/depth. |
| gpu.ssao | HDR + normals/depth → HDR с ambient occlusion. |
| gpu.tonemap | HDR → linear LDR; режимы ACES/Reinhard, exposure. |
| gpu.fxaa | Linear LDR → Linear LDR, spatial AA текущего кадра. |
| gpu.color_encode | Linear LDR → rgba8unorm/srgb_display. |
| gpu.custom | Собственный raster/compute проход с params.manifest и source_id. |

## Редактирование в Engine

Выберите «GPU Forward» или «GPU Deferred», затем откройте вложенный граф renderer двойным щелчком. Выберите проход: инспектор показывает его параметры и контракты ресурсов. «Заменить на редактируемый Custom HLSL» копирует поставляемый текст шейдера и manifest в проект, сохраняя ID прохода и его внешние связи.

Во вкладке «HLSL» редактируйте текст и нажмите «Применить HLSL». Неверный кандидат остаётся в проекте с диагностикой; предыдущая рабочая конфигурация сохраняется. Изменять ресурсы, bindings и точки входа нужно согласованно с manifest.

Самостоятельные шаблоны находятся в assets/gpu/custom_compute.hlsl, custom_raster.hlsl и custom_fullscreen.hlsl. Они доступны как обычные исходники.

![Вложенный граф GPU-проходов](media/editor-render-v02.png)

## Manifest прохода

Пример params для HDR compute-фильтра:

~~~json
{
  "source_id": "my_filter",
  "gain": 1.0,
  "manifest": {
    "stage": "compute",
    "entry": "main",
    "workgroup": [8, 8, 1],
    "parameters": ["gain"],
    "inputs": [{
      "name": "color", "format": "rgba32float",
      "semantic": "hdr_linear", "extent": "viewport",
      "samples": 1, "access": "sampled_read"
    }],
    "outputs": [{
      "name": "color", "format": "rgba32float",
      "semantic": "hdr_linear", "extent": "viewport",
      "samples": 1, "access": "storage_write"
    }]
  }
}
~~~

source_id указывает на shader_sources. stage — compute или raster. Для raster raster_mode выбирает scene/fullscreen; entry задаёт fragment entry, vertex_entry — необязательную пользовательскую vertex entry. scene raster принимает ровно один Scene3D, fullscreen/compute его не принимают.

ResourceSpec содержит name, format, semantic, extent, samples, access. Форматы изображений: rgba32float, r32float, rgba8unorm. semantics встроенных проходов: hdr_linear, ldr_linear, albedo_linear, world_normals, camera_linear_depth, srgb_display; сцена имеет format:scene/semantic:scene3d. Входные изображения используют sampled_read, выход raster — color_write, compute — storage_write. Совместимость требует совпадения формата, semantic, extent и samples.

Неизвестные поля manifest/ResourceSpec отклоняются. params прохода допускает только объявленные параметры, enabled:bool и служебные source_id/manifest у Custom. До 16 scalar-параметров; порядок parameters определяет размещение значений в push constants. Встроенный tonemap отдельно поддерживает строковый mode.

## HLSL ABI

Descriptor set 0: сначала входные bindings в порядке inputs, затем выходные compute bindings в порядке outputs. Texture2D<float> и Texture2D<float4> читаются через Load; sampler/buffer bindings в этом manifest пока отсутствуют. У storage image требуется точный атрибут vk::image_format, например rgba32f. Reflection проверяет скомпилированные image type, bindings, storage format и LocalSize.

Push constants — 128 байт: четыре float4 камеры и четыре float4 параметров. Используйте поставляемое объявление [[vk::push_constant]] ConstantBuffer<Config>; параметры занимают values, например первый scalar — values[0].x.

Compute workgroup должен совпадать с numthreads в SPIR-V, z=1, произведение размеров — от 1 до 256. Проверяйте границы изображения до Store. Для scene vertex ABI: position float3, normal float3, color float4, ids float2, UV float2, atlasRect float4, locations 0–5, stride 72 байта. Первые 48 байт сохраняют прежнее расположение данных: существующий Custom шейдер может использовать только locations 0–3. Пример vertex/fragment пары находится в custom_raster.hlsl.

Опциональное поле manifest `scene_materials: true` разрешено только для scene raster. Оно добавляет sampled image binding после явно перечисленных image inputs — base-color атлас из материалов `SceneSnapshot`. В Forward/G-buffer это binding 0. UV находится в location 4, atlasRect — в location 5; rect содержит начало и размер текстуры в пикселях. Fragment шейдер переносит UV с перспективной интерполяцией и rect с `nointerpolation`, затем использует `Texture2D.Load`. Встроенные шейдеры явно декодируют sRGB в линейный цвет. Преобразование встроенного прохода в Custom сохраняет этот manifest и material ABI. Depth/normal и обычный Custom не получают атлас без явного включения. Подробности импорта: [content.md](content.md).

Inline HLSL ищет буквальные #include в assets/gpu выбранного пакета, затем в корне этого пакета. Для вложенного quoted include первым ищется каталог содержащего файла; angle include использует assets/gpu→root. JSON из отдельного Full/Team пакета использует его marker/resource root. Самостоятельный JSON без marker не делает свой каталог новым compiler root; source_id не является путём. Экспорт рекурсивно собирает эти зависимости. Содержимое разрешённых include читается перед выбором pipeline и входит в fingerprint: изменение header инвалидирует compile/pipeline cache даже при неизменном inline тексте. Это проверено изменением реальных GPU-пикселей. Макрос вместо имени include отклоняется при сборе зависимостей, чтобы экспорт не терял неизвестные файлы.

Готовые ACES/Reinhard и FXAA/off можно открыть автору Project как разрешённые варианты; Team не изменяет граф/HLSL. DLAA/SMAA пока отсутствуют.

## Заранее скомпилированный Player и кеши

`precompile_graph_at(graph, sources, source_root, destination)` компилирует и проверяет все стадии выбранного графа, включая vertex, и пишет `destination/.spirv`. При наличии `.spirv` renderer использует только эти артефакты: DXC не запускается и отката к инструментам автора нет. Отсутствующий/повреждённый shader приводит к диагностике и требует повторного экспорта. Include-файлы сохраняются в пакете, потому что их содержимое проверяется тем же fingerprint. Для других разрешённых графов экспортёр вызывает precompile отдельно в тот же каталог.

В редакторе устойчивый `.work/shader-cache` повторно использует SPIR-V по исходнику, entry/profile и содержимому include closure. Полный артефакт публикуется переименованием временного файла; незавершённый результат не должен читаться параллельным компилятором.

Backend сохраняет изображения проходов, vertex buffer и readback buffer между кадрами одинаковой конфигурации/размера. GPU material atlas загружается при изменении набора декодированных текстур и используется повторно. `set_detailed_stats(false)` применяется Player: на CPU возвращается только RGBA8 вместо дополнительного чтения depth, normals и цветовых ID. В таком режиме ownership/coverage counts не измеряются; ноль в этих полях не означает невидимую сцену. Submitted counts остаются доступны.

Идентичность ресурсов включает ID проходов и их выходов, а не только набор pipelines: удаление AO и добавление такого же прохода с другим ID требует нового набора изображений. Иначе повторно использованный `ao.color` не соответствует новому `pass_id.color`. GPU-план проверяется отдельно от кеша изображений; контракты настроек проверяются по metadata без повторного чтения шейдеров. Для неизменённого кадра план переиспользуется, а внешние includes проверяются с интервалом 250 мс; явная подготовка всегда читает актуальные includes.

Readback buffer предпочитает память `HOST_VISIBLE | HOST_COHERENT | HOST_CACHED`. Просто первая host-visible память дискретной видеокарты может оказаться медленно читаемым CPU BAR mapping. Если cached coherent тип недоступен, backend использует прежний compatible host-visible coherent тип. Барьер transfer→host и завершение очереди сохраняются.

Renderer отдельно измеряется командой `cargo run --locked --release -p module-engine-vulkan --example gpu_benchmark`: 20 импортированных текстурированных моделей, полная стандартная цепочка Forward/SSAO/tonemap/FXAA/encoding, 1920×1080, 10 кадров прогрева и 120 измеряемых кадров. На RTX 4080 SUPER после исправления readback memory получено в среднем 3.11 ms, p95 3.98 ms. Это диагностический замер renderer с CPU readback, без игровых подсистем и вывода egui; критерию готового Player соответствует отдельный полный замер. JSON пишется в `.work/gpu-benchmark.json`.

## Ограничения и требования

Нужен Vulkan 1.3, dynamicRendering, очередь с graphics+compute, host-visible coherent память и device-local изображения. Форматы RGBA32_SFLOAT/R32_SFLOAT/RGBA8_UNORM должны поддерживать используемые color attachment/sample/storage/transfer операции; D32_SFLOAT — depth attachment/transfer src. Наличие vulkan-1.dll не доказывает возможности адаптера: их проверяет реальное создание backend.

Изображения viewport однослойные, samples=1; максимум 4096 по каждой оси и 4 млн пикселей. MSAA/resolve и temporal AA отложены. Native swapchain отсутствует: кадр читается синхронно для egui.

Clear/copy команды адресуют pass_id.output_name и выполняются после producer в порядке отправки. Неверный ресурс, несовместимый copy или запись после первого consumer отклоняются. enabled override переносимого Rust имеет приоритет над params.enabled; выключение допустимо только при совместимом одиночном image bypass.

Для встроенной геометрии renderer читает видимые ID и измеряет ownership. У произвольного custom scene шейдера доступны submitted triangles и coverage; нулевые ownership counts означают отсутствие такого измерения, а не доказательство отсутствия видимой геометрии.

