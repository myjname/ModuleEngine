<a id="статические-модели-текстуры-и-материалы"></a>

# Static Models, Textures, and Materials

[English](content.md) | [Русский](ru/content.md) | [Documentation](README.md)

Content is loaded by a separate `module-engine-content` library. The world stores objects and their stable IDs; the library attaches geometry and materials to the extracted `SceneSnapshot`. Object and ECS therefore use the same files, and the renderer receives one neutral structure.

<a id="что-загружается"></a>

## What Is Loaded

Supported formats are glTF 2.0 `.gltf` and `.glb`: indexed and nonindexed triangle primitives, a static node hierarchy, their translation/rotation/scale or matrix, UV set 0, `baseColorFactor`, and `baseColorTexture`. External `.bin`, PNG/JPEG, embedded bufferView images, and base64 data URIs are actually read. A composite mesh retains separate primitives and materials. glTF column-major matrices are converted to the engine's row-major convention and combined with the game object's transform.

This is limited static content import, not a claim of full glTF conformance. Skins, animations, morph targets, sparse accessors, nontriangle primitives, required extensions, transparent materials, and texturing through another UV set are rejected with a message. Metallic, roughness, normal/emissive/occlusion maps do not yet contribute to lighting. The renderer uses a flat triangle normal and simple built-in lighting.

PNG/JPEG can be loaded separately and assigned to existing geometry. Base-color texture RGB is treated as sRGB and converted to linear color in the GPU shader. The material factor and object color multiply the result. Current sampling is nearest with repeat; arbitrary glTF sampler/filter modes and mipmaps are not yet executed.

<a id="ссылка-из-сцены"></a>

## Scene Reference

In the components of an authored object or template instance:

~~~json
{
  "mesh": {"path": "assets/models/demo_crystal.glb"},
  "material": {
    "base_color": [1.0, 1.0, 1.0, 1.0],
    "base_color_texture": "assets/textures/stone.png"
  }
}
~~~

The `material` component is optional: glTF materials are used without it. Override fields are also optional. Unchanged built-in objects use cubes; an imported object draws its own vertices and triangles. `demo_crystal.glb` is a small original test crystal with an embedded PNG texture.

<a id="api-и-жизненный-цикл"></a>

## API and Lifecycle

`ContentLibrary::new(package_root)` fixes the root of a particular project/package. `load_model(path)` returns a cached `Arc<LoadedModel>`. `enrich_snapshot(snapshot, bindings)` replaces cubes for objects with the specified IDs with model primitives, preserving the object's transform and color. Multiple primitives have the original object ID. `load_texture(path)` returns a cached `Arc<TextureImage>` for a material override. Textures are shared between snapshots through Arc; they are not decoded again every frame.

`invalidate()` clears the decoded cache after resource changes. Opening another game creates another library with its own root. Switching the world model does not change file-reading rules or the material ABI.

`import_model(source, destination_directory)` validates the model and all relative dependencies, then copies them while preserving their structure. The import source root is the selected model's directory; dependencies outside that directory are rejected during import. If the source uses a neighboring directory through `../`, first place the resource set in the project and load it from the project root. `import_texture(source, destination_filename)` first decodes PNG/JPEG and only then writes the file into the project.

<a id="владение-файлами-и-экспорт"></a>

## File Ownership and Export

Game resource paths are relative to the selected package. Absolute paths and URLs are not used. External glTF URIs are resolved relative to the glTF directory; percent-encoding is decoded before validation. The canonical path must remain inside the package, including symbolic link targets. During loading, relative `../` is allowed only if the result remains inside the project root. Copying also checks an existing file target.

`dependencies(model_paths)` returns the complete set of model, buffer, and image files for export. For a `.glb` with all content embedded, this is one file; for an external `.gltf`, the document and its actual dependencies. The exporter adds separate material override textures from component references. Player uses its own files and does not access the authored checkout.

Loading limits: 128 MiB per model/buffer/encoded texture file; textures up to 4096×4096, decoding limit 128 MiB; up to one million vertices per primitive and three million accessor elements; up to 4096 nodes and 128 hierarchy levels. Binary ranges are validated before iterator creation. The current renderer's GPU atlas allows up to 8192 on each axis. A large game needs streaming, mipmaps, and a broader material/resource contract.

<a id="проверка"></a>

## Verification

~~~powershell
cargo test --locked -p module-engine-content -p module-engine-vulkan
~~~

Tests import the bundled GLB, an external glTF with `.bin` and PNG, copy the complete set, and load a standalone JPEG. They check node matrices and rejection of external/encoded path traversal. A separate Vulkan test confirms that the imported mesh's UVs and texture change real Forward and Deferred pixels; a repeated frame after cache use matches.

The format is implemented from the [Khronos glTF 2.0 specification](https://github.com/KhronosGroup/glTF/tree/main/specification/2.0); the supported-feature list above defines the actual boundary of this version.
