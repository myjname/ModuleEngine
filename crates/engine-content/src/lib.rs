//! Static glTF content import with explicit package ownership and a neutral scene.
//! No glTF document or GPU handle is required by either world implementation.
use base64::Engine;
use module_engine_core::{
    Material, Mesh, RenderItem, SceneSnapshot, TextureImage, identity_matrix, multiply_matrices,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// One primitive and its glTF node transform, relative to the authored game object.
#[derive(Clone, Debug)]
pub struct ModelPrimitive {
    pub mesh: Mesh,
    pub material: Material,
    pub transform: [[f32; 4]; 4],
}
/// Import result. Dependencies are relative to the selected package, for export.
#[derive(Clone, Debug)]
pub struct LoadedModel {
    pub primitives: Vec<ModelPrimitive>,
    pub dependencies: BTreeSet<String>,
}
/// A package-scoped decoded asset cache. Call invalidate after changing source files.
pub struct ContentLibrary {
    root: PathBuf,
    asset_paths: BTreeMap<String, String>,
    models: BTreeMap<String, Arc<LoadedModel>>,
    textures: BTreeMap<String, Arc<TextureImage>>,
}
impl ContentLibrary {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|e| format!("Asset root: {e}"))?;
        Ok(Self {
            root,
            asset_paths: BTreeMap::new(),
            models: BTreeMap::new(),
            textures: BTreeMap::new(),
        })
    }
    pub fn invalidate(&mut self) {
        self.models.clear();
        self.textures.clear();
    }
    /// Install the project's stable asset index. A changed mapping invalidates
    /// decoded files so moved assets never reuse an obsolete dependency root.
    pub fn set_asset_paths(&mut self, asset_paths: BTreeMap<String, String>) {
        if self.asset_paths != asset_paths {
            self.asset_paths = asset_paths;
            self.invalidate();
        }
    }
    pub fn resolve_reference(&self, reference: &str) -> Result<String, String> {
        if let Some(id) = reference.strip_prefix("asset://") {
            self.asset_paths
                .get(id)
                .cloned()
                .ok_or_else(|| format!("Unknown asset ID: {id}"))
        } else {
            Ok(reference.into())
        }
    }
    pub fn load_model(&mut self, path: &str) -> Result<Arc<LoadedModel>, String> {
        let resolved = self.resolve_reference(path)?;
        let path = resolved.as_str();
        if let Some(model) = self.models.get(path) {
            return Ok(model.clone());
        }
        let file = owned_path(&self.root, path)?;
        let model = Arc::new(load_model(&self.root, &file)?);
        self.models.insert(path.into(), model.clone());
        Ok(model)
    }
    /// Load a standalone PNG/JPEG for an authored material override.
    pub fn load_texture(&mut self, path: &str) -> Result<Arc<TextureImage>, String> {
        let resolved = self.resolve_reference(path)?;
        let path = resolved.as_str();
        if let Some(texture) = self.textures.get(path) {
            return Ok(texture.clone());
        }
        let file = owned_path(&self.root, path)?;
        let bytes = std::fs::read(file).map_err(|e| format!("Texture {path}: {e}"))?;
        let texture = decode_texture(bytes)?;
        self.textures.insert(path.into(), texture.clone());
        Ok(texture)
    }
    /// Copy one validated PNG/JPEG to a project-relative filename.
    pub fn import_texture(&mut self, source: &Path, destination: &str) -> Result<String, String> {
        validate_relative(destination)?;
        let bytes = std::fs::read(source).map_err(|e| format!("Texture import: {e}"))?;
        decode_texture(bytes.clone())?;
        let target = self.root.join(destination);
        let parent = target.parent().ok_or("Texture destination has no parent")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        if !parent
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(&self.root)
            || (target.exists()
                && !target
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(&self.root))
        {
            return Err("Texture destination leaves project".into());
        }
        std::fs::write(target, bytes).map_err(|e| e.to_string())?;
        self.invalidate();
        Ok(destination.into())
    }
    /// Attach authored mesh paths by stable object ID, independently of world storage.
    /// Multiple primitives retain the original object ID for picking and statistics.
    pub fn enrich_snapshot(
        &mut self,
        snapshot: &mut SceneSnapshot,
        bindings: &BTreeMap<u64, String>,
    ) -> Result<(), String> {
        let mut result = vec![];
        for item in &snapshot.items {
            if let Some(path) = bindings.get(&item.id) {
                let model = self.load_model(path)?;
                for primitive in &model.primitives {
                    result.push(RenderItem {
                        id: item.id,
                        mesh: primitive.mesh.clone(),
                        model: multiply_matrices(item.model, primitive.transform),
                        color: item.color,
                        material: primitive.material.clone(),
                    });
                }
            } else {
                result.push(item.clone());
            }
        }
        snapshot.items = result;
        Ok(())
    }
    /// Validated transitive files needed for copying a model to a Player package.
    pub fn dependencies(
        &mut self,
        paths: impl IntoIterator<Item = String>,
    ) -> Result<BTreeSet<String>, String> {
        let mut result = BTreeSet::new();
        for path in paths {
            result.extend(self.load_model(&path)?.dependencies.iter().cloned());
        }
        Ok(result)
    }
    /// Import a model and its relative buffers/images into one project directory.
    /// The source model's directory bounds this import; outside references are rejected.
    pub fn import_model(&mut self, source: &Path, destination: &str) -> Result<String, String> {
        validate_relative(destination)?;
        let source = source
            .canonicalize()
            .map_err(|e| format!("Import source: {e}"))?;
        let source_root = source.parent().ok_or("Import source has no parent")?;
        let model = load_model(source_root, &source)?;
        let target = self.root.join(destination);
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let target = target.canonicalize().map_err(|e| e.to_string())?;
        if !target.starts_with(&self.root) {
            return Err("Import target leaves project".into());
        }
        // Validate every dependency before writing any files.
        let sources = model
            .dependencies
            .iter()
            .map(|p| owned_path(source_root, p).map(|s| (p.clone(), s)))
            .collect::<Result<Vec<_>, _>>()?;
        for (relative, from) in sources {
            let to = target.join(relative);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            if let Some(parent) = to.parent() {
                let parent = parent.canonicalize().map_err(|e| e.to_string())?;
                if !parent.starts_with(&target) {
                    return Err("Import dependency target leaves project".into());
                }
            }
            if to.exists()
                && !to
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(&target)
            {
                return Err("Import dependency file leaves project".into());
            }
            if from != to {
                std::fs::copy(from, to).map_err(|e| format!("Import copy: {e}"))?;
            }
        }
        self.invalidate();
        Ok(format!(
            "{}/{}",
            destination.trim_end_matches('/'),
            source.file_name().unwrap().to_string_lossy()
        ))
    }
}
fn validate_relative(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.contains(':')
        || path.contains('\\')
        || Path::new(path).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "Asset path must be a project-relative path: {path}"
        ));
    }
    Ok(())
}
fn owned_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    validate_relative(path)?;
    let file = root
        .join(path)
        .canonicalize()
        .map_err(|e| format!("Asset {path}: {e}"))?;
    if !file.starts_with(root) || !file.is_file() {
        return Err(format!("Asset {path} leaves project or is not a file"));
    }
    Ok(file)
}
fn uri_bytes(
    root: &Path,
    parent: &Path,
    uri: &str,
    dependencies: &mut BTreeSet<String>,
) -> Result<Vec<u8>, String> {
    if let Some(data) = uri.strip_prefix("data:") {
        let (header, data) = data.split_once(',').ok_or("Malformed asset data URI")?;
        if !header.ends_with(";base64") {
            return Err("Asset data URI must be base64".into());
        }
        return base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("Asset base64: {e}"));
    }
    // URI percent escapes are decoded before validation; encoded traversal cannot bypass ownership.
    let mut decoded = vec![];
    let bytes = uri.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3).ok_or("Malformed URI escape")?)
                .map_err(|e| e.to_string())?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|e| e.to_string())?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let uri = String::from_utf8(decoded).map_err(|e| e.to_string())?;
    if uri.is_empty() || uri.contains(':') || uri.contains('\\') || Path::new(&uri).is_absolute() {
        return Err("Asset URI must be a relative project file".into());
    }
    let file = parent
        .join(&uri)
        .canonicalize()
        .map_err(|e| format!("Asset dependency {uri}: {e}"))?;
    if !file.starts_with(root) || !file.is_file() {
        return Err(format!("Asset dependency leaves project: {uri}"));
    }
    dependencies.insert(relative(root, &file)?);
    std::fs::read(file).map_err(|e| e.to_string())
}
fn relative(root: &Path, file: &Path) -> Result<String, String> {
    Ok(file
        .strip_prefix(root)
        .map_err(|_| "Asset outside root")?
        .to_string_lossy()
        .replace('\\', "/"))
}
fn decode_texture(bytes: Vec<u8>) -> Result<Arc<TextureImage>, String> {
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("Encoded texture exceeds 128 MiB".into());
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if !matches!(
        reader.format(),
        Some(image::ImageFormat::Png | image::ImageFormat::Jpeg)
    ) {
        return Err("Textures must be PNG or JPEG".into());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|e| format!("Texture decode: {e}"))?
        .into_rgba8();
    Ok(Arc::new(TextureImage {
        width: decoded.width(),
        height: decoded.height(),
        rgba8: decoded.into_raw(),
    }))
}
fn load_model(root: &Path, path: &Path) -> Result<LoadedModel, String> {
    if !matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some("gltf" | "glb")
    ) {
        return Err("Models must be glTF 2.0 .gltf or .glb".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 128 * 1024 * 1024 {
        return Err("Model file exceeds 128 MiB".into());
    }
    let gltf =
        gltf::Gltf::from_slice(&bytes).map_err(|e| format!("glTF {}: {e}", path.display()))?;
    if gltf.extensions_required().next().is_some() {
        return Err("Required glTF extensions are not supported by static importer".into());
    }
    if gltf.skins().next().is_some() || gltf.animations().next().is_some() {
        return Err("Static importer does not support skins or animations".into());
    }
    let mut dependencies = BTreeSet::from([relative(root, path)?]);
    let parent = path.parent().unwrap();
    let buffers = gltf
        .buffers()
        .map(|b| {
            let bytes = match b.source() {
                gltf::buffer::Source::Bin => gltf.blob.clone().ok_or("GLB binary chunk missing")?,
                gltf::buffer::Source::Uri(uri) => uri_bytes(root, parent, uri, &mut dependencies)?,
            };
            if bytes.len() < b.length() {
                return Err("glTF buffer shorter than declared byteLength".into());
            }
            if bytes.len() > 128 * 1024 * 1024 {
                return Err("glTF buffer exceeds 128 MiB".into());
            }
            Ok(bytes)
        })
        .collect::<Result<Vec<_>, String>>()?;
    // Validate binary ranges before utility iterators perform arithmetic/slicing.
    for view in gltf.views() {
        let end = view
            .offset()
            .checked_add(view.length())
            .ok_or("glTF bufferView offset overflow")?;
        if end > view.buffer().length() {
            return Err("glTF bufferView exceeds declared buffer".into());
        }
    }
    for accessor in gltf.accessors() {
        if accessor.sparse().is_some() {
            return Err("Sparse glTF accessors are not supported by static importer".into());
        }
        if accessor.count() > 3_000_000 {
            return Err("glTF accessor exceeds three million elements".into());
        }
        let view = accessor.view().ok_or("glTF accessor has no bufferView")?;
        let stride = view.stride().unwrap_or(accessor.size());
        let bytes = accessor
            .count()
            .saturating_sub(1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(accessor.size()))
            .and_then(|n| n.checked_add(accessor.offset()))
            .ok_or("glTF accessor range overflow")?;
        if bytes > view.length() {
            return Err("glTF accessor exceeds bufferView".into());
        }
    }
    let images = gltf
        .images()
        .map(|image| {
            let bytes = match image.source() {
                gltf::image::Source::Uri { uri, .. } => {
                    uri_bytes(root, parent, uri, &mut dependencies)?
                }
                gltf::image::Source::View { view, .. } => buffers
                    .get(view.buffer().index())
                    .and_then(|b| b.get(view.offset()..view.offset().checked_add(view.length())?))
                    .ok_or("Image bufferView out of range")?
                    .to_vec(),
            };
            decode_texture(bytes)
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut result = LoadedModel {
        primitives: vec![],
        dependencies,
    };
    let scene = gltf
        .default_scene()
        .or_else(|| gltf.scenes().next())
        .ok_or("glTF has no scene")?;
    let mut seen = BTreeSet::new();
    for node in scene.nodes() {
        extract_node(
            node,
            identity_matrix(),
            &buffers,
            &images,
            &mut seen,
            &mut result,
            0,
        )?;
    }
    if result.primitives.is_empty() {
        return Err("glTF scene has no triangle geometry".into());
    }
    Ok(result)
}
fn extract_node(
    node: gltf::Node<'_>,
    parent: [[f32; 4]; 4],
    buffers: &[Vec<u8>],
    images: &[Arc<TextureImage>],
    seen: &mut BTreeSet<usize>,
    result: &mut LoadedModel,
    depth: usize,
) -> Result<(), String> {
    if depth > 128 {
        return Err("glTF hierarchy exceeds 128 levels".into());
    }
    if !seen.insert(node.index()) {
        return Err("glTF node graph contains a cycle or shared child".into());
    }
    if seen.len() > 4096 {
        return Err("glTF scene exceeds 4096 nodes".into());
    }
    // glTF stores column-major; engine's neutral transform is row-major.
    let source = node.transform().matrix();
    let local = std::array::from_fn(|r| std::array::from_fn(|c| source[c][r]));
    let transform = multiply_matrices(parent, local);
    if !transform.iter().flatten().all(|v| v.is_finite()) {
        return Err("glTF node has nonfinite transform".into());
    }
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                return Err("Static importer supports triangle lists only".into());
            }
            if primitive.morph_targets().next().is_some() {
                return Err("Morph targets are not supported by static importer".into());
            }
            let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
            let vertices = reader
                .read_positions()
                .ok_or("Primitive POSITION missing or invalid")?
                .collect::<Vec<_>>();
            if vertices.len() > 1_000_000 || !vertices.iter().flatten().all(|v| v.is_finite()) {
                return Err("Invalid or excessive model vertices".into());
            }
            let indices = reader
                .read_indices()
                .map(|v| v.into_u32().map(|i| i as usize).collect::<Vec<_>>())
                .unwrap_or_else(|| (0..vertices.len()).collect());
            if indices.len() % 3 != 0 || indices.iter().any(|i| *i >= vertices.len()) {
                return Err("Primitive triangle index out of range".into());
            }
            let triangles = indices.as_chunks::<3>().0.to_vec();
            let texcoords = reader
                .read_tex_coords(0)
                .map(|v| v.into_f32().collect::<Vec<_>>())
                .unwrap_or_default();
            if (!texcoords.is_empty() && texcoords.len() != vertices.len())
                || !texcoords.iter().flatten().all(|v| v.is_finite())
            {
                return Err("Primitive UVs invalid".into());
            }
            let material = primitive.material();
            if material.alpha_mode() != gltf::material::AlphaMode::Opaque {
                return Err("Static importer currently requires opaque materials".into());
            }
            let pbr = material.pbr_metallic_roughness();
            let base_color = pbr.base_color_factor();
            let base_color_texture = if let Some(info) = pbr.base_color_texture() {
                if info.tex_coord() != 0 {
                    return Err("Only material UV set zero is supported".into());
                }
                Some(
                    images
                        .get(info.texture().source().index())
                        .ok_or("Material image missing")?
                        .clone(),
                )
            } else {
                None
            };
            result.primitives.push(ModelPrimitive {
                mesh: Mesh {
                    vertices,
                    triangles,
                    texcoords,
                },
                material: Material {
                    base_color,
                    base_color_texture,
                },
                transform,
            });
        }
    }
    for child in node.children() {
        extract_node(child, transform, buffers, images, seen, result, depth + 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_ids_load_models_and_mapping_changes_invalidate_old_dependency_paths() {
        let root = std::env::temp_dir().join(format!(
            "content-stable-id-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(
            root.join("assets/old.glb"),
            include_bytes!("../../../assets/models/demo_crystal.glb"),
        )
        .unwrap();
        let mut library = ContentLibrary::new(&root).unwrap();
        library.set_asset_paths(BTreeMap::from([(
            "model-id".into(),
            "assets/old.glb".into(),
        )]));
        let old = library.load_model("asset://model-id").unwrap();
        assert!(Arc::ptr_eq(
            &old,
            &library.load_model("assets/old.glb").unwrap()
        ));
        std::fs::rename(root.join("assets/old.glb"), root.join("assets/new.glb")).unwrap();
        library.set_asset_paths(BTreeMap::from([(
            "model-id".into(),
            "assets/new.glb".into(),
        )]));
        let new = library.load_model("asset://model-id").unwrap();
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(new.dependencies, BTreeSet::from(["assets/new.glb".into()]));
        assert!(library.load_model("asset://missing").is_err());
        std::fs::remove_file(root.join("assets/new.glb")).unwrap();
        std::fs::remove_dir(root.join("assets")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn ownership_rejects_absolute_and_encoded_escape() {
        let root = std::env::temp_dir().join(format!("module-content-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(owned_path(&root, "../other.glb").is_err());
        assert!(owned_path(&root, "C:/other.glb").is_err());
        assert!(uri_bytes(&root, &root, "%2e%2e/other.bin", &mut BTreeSet::new()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn bundled_model_has_real_geometry_uv_texture_and_dependencies() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut library = ContentLibrary::new(root).unwrap();
        let model = library
            .load_model("assets/models/demo_crystal.glb")
            .unwrap();
        assert!(model.primitives[0].mesh.vertices.len() > 3);
        assert!(!model.primitives[0].mesh.texcoords.is_empty());
        assert!(model.primitives[0].material.base_color_texture.is_some());
        assert_eq!(model.dependencies.len(), 1);
        let mut snapshot = module_engine_core::ObjectsWorld::demo();
        use module_engine_core::WorldProvider;
        let mut scene = snapshot.snapshot();
        library
            .enrich_snapshot(
                &mut scene,
                &BTreeMap::from([(1, "assets/models/demo_crystal.glb".into())]),
            )
            .unwrap();
        assert_ne!(scene.items[0].mesh, Mesh::cube());
        snapshot.step(0., 0.);
    }
    #[test]
    fn external_model_import_copies_dependency_closure_and_standalone_png_jpeg_are_real_images() {
        let temp =
            std::env::temp_dir().join(format!("module-content-external-{}", std::process::id()));
        let source = temp.join("source");
        let project = temp.join("project");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let glb = include_bytes!("../../../assets/models/demo_crystal.glb");
        let jsonlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let mut json: serde_json::Value = serde_json::from_slice(&glb[20..20 + jsonlen]).unwrap();
        let binary = &glb[20 + jsonlen + 8..];
        let imageview = json["images"][0]["bufferView"].as_u64().unwrap() as usize;
        let view = &json["bufferViews"][imageview];
        let offset = view["byteOffset"].as_u64().unwrap() as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        std::fs::write(source.join("checker.png"), &binary[offset..offset + length]).unwrap();
        std::fs::write(source.join("model.bin"), binary).unwrap();
        json["buffers"][0]["uri"] = serde_json::json!("model.bin");
        json["images"][0] = serde_json::json!({"uri":"checker.png"});
        json["nodes"][0]["translation"] = serde_json::json!([3., 0., 0.]);
        std::fs::write(
            source.join("model.gltf"),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
        let mut library = ContentLibrary::new(&project).unwrap();
        let imported = library
            .import_model(&source.join("model.gltf"), "assets/imported/crystal")
            .unwrap();
        let model = library.load_model(&imported).unwrap();
        assert_eq!(model.dependencies.len(), 3);
        assert_eq!(model.primitives[0].transform[0][3], 3.);
        for file in &model.dependencies {
            assert!(project.join(file).is_file());
        }
        let png = library
            .load_texture("assets/imported/crystal/checker.png")
            .unwrap();
        assert_eq!(png.width, 8);
        assert_eq!(png.height, 8);
        assert!(Arc::ptr_eq(
            &png,
            &library
                .load_texture("assets/imported/crystal/checker.png")
                .unwrap()
        ));
        let jpeg = source.join("own.jpg");
        image::RgbImage::from_pixel(2, 3, image::Rgb([180, 40, 70]))
            .save_with_format(&jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        library
            .import_texture(&jpeg, "assets/textures/own.jpg")
            .unwrap();
        let decoded = library.load_texture("assets/textures/own.jpg").unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 3));
        assert!(decoded.rgba8[0] > 150);
        json["buffers"][0]["uri"] = serde_json::json!("../../outside.bin");
        std::fs::write(
            source.join("escape.gltf"),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
        assert!(
            library
                .import_model(&source.join("escape.gltf"), "assets/imported/bad")
                .is_err()
        );
        assert!(!project.join("assets/imported/bad").exists());
        std::fs::remove_dir_all(temp).unwrap();
    }
}
