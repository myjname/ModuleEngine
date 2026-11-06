# Crate Garden assets

[English](README.md) | [Русский](README.ru.md) | [Game guide](../../docs/g1-game.md)

This directory contains original low-poly models, a palette texture, short sound
effects and a quiet looping music track for G1. The game rules, original ten
boards, presentation and grid palette belong to
[`module-engine-g1`](../../crates/project-g1/README.md). Reusable algorithms,
audio output and persistence belong to separate engine packages.

Create an editable project using the normal Full Editor command:

```powershell
ModuleEngine.exe --new-g1 E:/Games/CrateGarden/project.json
```

Use a new project filename and destination. The normal transaction copies the
referenced assets, indexes them and publishes the project with its selected
Engine profile. Open that JSON in Editor, or export a standalone Player through
File/Tools or `--export-game`. Engine authoring still requires the complete
matching Full source package; this asset directory does not grant that capability.

`generate_assets.py` uses only Python's standard library. It deterministically
creates six static glTF meshes with embedded geometry and one shared PNG, plus
eight mono PCM16 WAV files. Run it from any directory:

```powershell
python examples/crate-garden/generate_assets.py
```

Existing different asset bytes are rejected. `--replace` explicitly regenerates
these assets; do not use it on a project containing your own edited replacements.
`asset-origins.json` records generated paths, sizes and SHA-256 values. All
models, sounds and the melody were created for this project by that script and
are distributed under the repository's [MIT license](../../LICENSE). No external
asset pack or recorded music is required.

The simple palette material intentionally targets the engine's currently
supported static opaque textured geometry. It does not demonstrate skeletal
animation, full PBR, streamed audio or a mobile platform.
