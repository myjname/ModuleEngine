use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
pub type AssetId = String;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetEntry {
    pub path: String,
    pub sha256: String,
    #[serde(default)]
    pub dependencies: Vec<AssetId>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AssetManifest {
    pub schema_version: u32,
    pub assets: BTreeMap<AssetId, AssetEntry>,
}
impl Default for AssetManifest {
    fn default() -> Self {
        Self {
            schema_version: 1,
            assets: BTreeMap::new(),
        }
    }
}
impl AssetManifest {
    /// Resolve portable asset references. Legacy relative paths remain valid.
    pub fn resolve_reference(&self, reference: &str) -> Result<String, String> {
        if let Some(id) = reference.strip_prefix("asset://") {
            let entry = self
                .assets
                .get(id)
                .ok_or_else(|| format!("Unknown asset ID: {id}"))?;
            normalize_relative(&entry.path)
        } else {
            normalize_relative(reference)
        }
    }
    /// Worker/runtime view of stable IDs, without duplicating manifest hashes.
    pub fn asset_paths(&self) -> BTreeMap<String, String> {
        self.assets
            .iter()
            .map(|(id, entry)| (id.clone(), entry.path.clone()))
            .collect()
    }
    pub fn from_value(value: &serde_json::Value) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self::default());
        }
        let manifest: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if manifest.schema_version != 1 {
            return Err("Unsupported asset manifest version".into());
        }
        Ok(manifest)
    }
    pub fn register(
        &mut self,
        id: impl Into<String>,
        relative: &str,
        project_root: &Path,
    ) -> Result<(), String> {
        let id = id.into();
        if id.trim().is_empty() || self.assets.contains_key(&id) {
            return Err("Asset ID is empty or already registered".into());
        }
        let path = normalize_relative(relative)?;
        let file = resolve_relative(project_root, &path)?;
        let sha256 = hash_file(&file)?;
        self.assets.insert(
            id,
            AssetEntry {
                path,
                sha256,
                dependencies: vec![],
            },
        );
        Ok(())
    }
    /// Expand literal HLSL include dependencies into pinned manifest entries.
    /// Existing IDs and declarations are retained; generated include IDs use
    /// the relative path and receive a suffix only on an ID collision.
    pub fn with_hlsl_dependencies(&self, project_root: &Path) -> Result<Self, String> {
        self.verify(project_root)?;
        let mut expanded = self.clone();
        for (id, entry) in &self.assets {
            let extension = Path::new(&entry.path)
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("");
            if !extension.eq_ignore_ascii_case("hlsl") && !extension.eq_ignore_ascii_case("hlsli") {
                continue;
            }
            for relative in collect_hlsl_dependencies(project_root, &entry.path)? {
                if normalize_relative(&entry.path)? == relative {
                    continue;
                }
                let dependency = if let Some((key, _)) =
                    expanded.assets.iter().find(|(_, asset)| {
                        normalize_relative(&asset.path).ok().as_ref() == Some(&relative)
                    }) {
                    key.clone()
                } else {
                    let base = format!("include:{relative}");
                    let mut key = base.clone();
                    let mut suffix = 1;
                    while expanded.assets.contains_key(&key) {
                        key = format!("{base}:{suffix}");
                        suffix += 1;
                    }
                    expanded.register(&key, &relative, project_root)?;
                    key
                };
                let dependencies = &mut expanded.assets.get_mut(id).unwrap().dependencies;
                if !dependencies.contains(&dependency) {
                    dependencies.push(dependency);
                }
            }
        }
        expanded.verify(project_root)?;
        Ok(expanded)
    }
    pub fn verify(&self, project_root: &Path) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("Unsupported asset manifest version".into());
        }
        for (id, entry) in &self.assets {
            if id.trim().is_empty() {
                return Err("Empty asset ID".into());
            }
            let file = resolve_relative(project_root, &entry.path)?;
            if entry.sha256.len() != 64
                || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit())
                || hash_file(&file)? != entry.sha256.to_ascii_lowercase()
            {
                return Err(format!(
                    "Asset {id}: content hash mismatch ({})",
                    entry.path
                ));
            }
            for dependency in &entry.dependencies {
                if !self.assets.contains_key(dependency) {
                    return Err(format!("Asset {id}: missing dependency {dependency}"));
                }
            }
        }
        Ok(())
    }
}
/// Normalize portable project-relative paths; parent traversal may not escape root.
pub fn normalize_relative(relative: &str) -> Result<String, String> {
    let portable = relative.replace('\\', "/");
    if portable.contains(':') || portable.starts_with('/') {
        return Err(format!(
            "Absolute/drive asset path is forbidden: {relative}"
        ));
    }
    let mut parts = vec![];
    for component in Path::new(&portable).components() {
        match component {
            Component::Normal(x) => parts.push(x.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.pop().is_none() {
                    return Err(format!("Path escapes project: {relative}"));
                }
            }
            _ => return Err(format!("Unsafe path: {relative}")),
        }
    }
    if parts.is_empty() {
        return Err("Empty asset path".into());
    }
    Ok(parts.join("/"))
}
pub fn resolve_relative(project_root: &Path, relative: &str) -> Result<PathBuf, String> {
    let root = project_root.canonicalize().map_err(|e| e.to_string())?;
    let relative = normalize_relative(relative)?;
    let resolved = root
        .join(&relative)
        .canonicalize()
        .map_err(|e| format!("{relative}: {e}"))?;
    if !resolved.starts_with(&root) || !resolved.is_file() {
        return Err(format!("Asset must be a file inside project: {relative}"));
    }
    Ok(resolved)
}
pub fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let count = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
/// Conservative dependency collection: literal includes, all branches, cycles deduplicated.
/// Macro include names fail explicitly rather than producing an incomplete export.
pub fn collect_hlsl_dependencies(project_root: &Path, entry: &str) -> Result<Vec<String>, String> {
    walk_hlsl_dependencies(project_root, vec![normalize_relative(entry)?])
}
/// Collect include dependencies of inline shader_sources without writing them into the source project.
/// virtual_path labels diagnostics; inline sources use DXC assets/gpu then project root, not source_id directories.
pub fn collect_hlsl_source_dependencies(
    project_root: &Path,
    virtual_path: &str,
    source: &str,
) -> Result<Vec<String>, String> {
    let virtual_path = normalize_relative(virtual_path)?;
    walk_hlsl_dependencies(
        project_root,
        hlsl_includes(project_root, &virtual_path, source, false)?,
    )
}
fn walk_hlsl_dependencies(
    project_root: &Path,
    mut pending: Vec<String>,
) -> Result<Vec<String>, String> {
    let mut visited = BTreeSet::new();
    let mut seen_files = BTreeSet::new();
    while let Some(relative) = pending.pop() {
        if !visited.insert(relative.clone()) {
            continue;
        }
        let file = resolve_relative(project_root, &relative)?;
        if !seen_files.insert(file.clone()) {
            continue;
        }
        let source = fs::read_to_string(file).map_err(|e| e.to_string())?;
        pending.extend(hlsl_includes(project_root, &relative, &source, true)?);
    }
    Ok(visited.into_iter().collect())
}
fn hlsl_includes(
    project_root: &Path,
    virtual_path: &str,
    source: &str,
    use_source_parent: bool,
) -> Result<Vec<String>, String> {
    let source = without_comments(source.trim_start_matches('\u{feff}'));
    let mut result = vec![];
    for line in source.lines() {
        let Some(directive) = line.trim_start().strip_prefix('#') else {
            continue;
        };
        let Some(rest) = directive.trim_start().strip_prefix("include") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace)
            && !rest.starts_with('"')
            && !rest.starts_with('<')
        {
            continue;
        }
        let rest = rest.trim();
        let close = if rest.starts_with('"') {
            '"'
        } else if rest.starts_with('<') {
            '>'
        } else {
            return Err(format!(
                "{virtual_path}: macro include cannot be exported safely: {rest}"
            ));
        };
        let end = rest[1..]
            .find(close)
            .ok_or_else(|| format!("{virtual_path}: malformed #include"))?
            + 1;
        let include = rest[1..end].replace('\\', "/");
        if include.starts_with('/') || include.contains(':') {
            return Err(format!(
                "{virtual_path}: absolute include is forbidden: {include}"
            ));
        }
        let parent = Path::new(virtual_path).parent().unwrap_or(Path::new(""));
        let candidate = [
            (use_source_parent && close == '"').then_some(parent),
            Some(Path::new("assets/gpu")),
            Some(Path::new("")),
        ]
        .into_iter()
        .flatten()
        .filter_map(|root| normalize_relative(&root.join(&include).to_string_lossy()).ok())
        .find(|path| resolve_relative(project_root, path).is_ok())
        .ok_or_else(|| format!("{virtual_path}: missing or out-of-project include: {include}"))?;
        result.push(candidate);
    }
    Ok(result)
}
fn without_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let (mut i, mut block, mut quote) = (0, false, false);
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if block {
            if c == '*' && next == Some('/') {
                block = false;
                i += 2;
                out.push(' ');
                continue;
            }
            if c == '\n' {
                out.push(c);
            }
            i += 1;
            continue;
        }
        if c == '"' && (i == 0 || chars[i - 1] != '\\') {
            quote = !quote;
            out.push(c);
            i += 1;
            continue;
        }
        if !quote && c == '/' && next == Some('*') {
            block = true;
            i += 2;
            out.push(' ');
            continue;
        }
        if !quote && c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExportedAsset {
    pub path: String,
    pub sha256: String,
}
/// Copies validated declared assets + recursive HLSL includes; preserves existing different files.
pub fn export_assets(
    project_root: &Path,
    manifest: &AssetManifest,
    destination: &Path,
) -> Result<Vec<ExportedAsset>, String> {
    manifest.verify(project_root)?;
    let mut files = BTreeMap::new();
    for entry in manifest.assets.values() {
        files.insert(
            normalize_relative(&entry.path)?,
            entry.sha256.to_ascii_lowercase(),
        );
        let ext = Path::new(&entry.path)
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or("");
        if ext.eq_ignore_ascii_case("hlsl") || ext.eq_ignore_ascii_case("hlsli") {
            for path in collect_hlsl_dependencies(project_root, &entry.path)? {
                let hash = hash_file(&resolve_relative(project_root, &path)?)?;
                files.entry(path).or_insert(hash);
            }
        }
    }
    fs::create_dir_all(destination).map_err(|e| e.to_string())?;
    let root = destination.canonicalize().map_err(|e| e.to_string())?;
    let mut report = vec![];
    for (relative, expected) in files {
        let source = resolve_relative(project_root, &relative)?;
        let target = root.join(&relative);
        let parent = target.parent().ok_or("Missing output parent")?;
        let mut ancestor = parent;
        while !ancestor.exists() {
            ancestor = ancestor.parent().ok_or("Missing output ancestor")?;
        }
        if !ancestor
            .canonicalize()
            .map_err(|e| e.to_string())?
            .starts_with(&root)
        {
            return Err("Destination symlink escapes export root".into());
        }
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        if target.exists() {
            if !target
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(&root)
                || hash_file(&target)? != expected
            {
                return Err(format!("Export would overwrite different file: {relative}"));
            }
        } else {
            let mut input = fs::File::open(source).map_err(|e| e.to_string())?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|e| e.to_string())?;
            std::io::copy(&mut input, &mut output)
                .and_then(|_| output.flush())
                .map_err(|e| e.to_string())?;
            if hash_file(&target)? != expected {
                return Err(format!("Asset changed during export: {relative}"));
            }
        }
        report.push(ExportedAsset {
            path: relative,
            sha256: expected,
        });
    }
    Ok(report)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recursive_includes_hashes_and_original_files_are_preserved() {
        let root =
            std::env::temp_dir().join(format!("asset-project-{}", crate::history::unique_id()));
        fs::create_dir_all(root.join("shaders/shared")).unwrap();
        fs::create_dir_all(root.join("textures")).unwrap();
        fs::write(
            root.join("shaders/main.hlsl"),
            "// #include \"missing\"\n#include \"shared/a.hlsli\"\n",
        )
        .unwrap();
        fs::write(
            root.join("shaders/shared/a.hlsli"),
            "# include \"../b.hlsli\"\n",
        )
        .unwrap();
        fs::write(root.join("shaders/b.hlsli"), "float x;\n").unwrap();
        fs::write(root.join("textures/a.bin"), [1, 2, 3]).unwrap();
        let mut manifest = AssetManifest::default();
        manifest
            .register("shader.main", "shaders/main.hlsl", &root)
            .unwrap();
        manifest
            .register("texture.main", "textures/a.bin", &root)
            .unwrap();
        let output = root.join("export");
        let report = export_assets(&root, &manifest, &output).unwrap();
        assert_eq!(report.len(), 4);
        assert_eq!(fs::read(output.join("textures/a.bin")).unwrap(), [1, 2, 3]);
        fs::write(output.join("textures/a.bin"), [9]).unwrap();
        assert!(export_assets(&root, &manifest, &output).is_err());
        assert_eq!(fs::read(output.join("textures/a.bin")).unwrap(), [9]);
        fs::write(root.join("textures/a.bin"), [7]).unwrap();
        assert!(manifest.verify(&root).is_err());
        assert!(resolve_relative(&root, "../elsewhere").is_err());
        assert!(resolve_relative(&root, "C:\\outside").is_err());
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn cycles_are_finite_and_macro_includes_are_explicit_errors() {
        let root =
            std::env::temp_dir().join(format!("hlsl-project-{}", crate::history::unique_id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.hlsl"), "#include \"b.hlsli\"\n").unwrap();
        fs::write(root.join("b.hlsli"), "#include \"a.hlsl\"\n").unwrap();
        assert_eq!(collect_hlsl_dependencies(&root, "a.hlsl").unwrap().len(), 2);
        fs::write(root.join("a.hlsl"), "#include HEADER\n").unwrap();
        assert!(collect_hlsl_dependencies(&root, "a.hlsl").is_err());
        crate::history::remove_test_directory(&root);
    }
    #[test]
    fn inline_shader_bom_and_dxc_include_root_are_exportable() {
        let root =
            std::env::temp_dir().join(format!("inline-shader-{}", crate::history::unique_id()));
        fs::create_dir_all(root.join("assets/gpu")).unwrap();
        fs::write(root.join("assets/gpu/header.hlsli"), "float value;\n").unwrap();
        fs::write(root.join("header.hlsli"), "float different_root_value;\n").unwrap();
        fs::create_dir_all(root.join("shaders")).unwrap();
        fs::write(
            root.join("shaders/header.hlsli"),
            "float different_virtual_value;\n",
        )
        .unwrap();
        let dependencies = collect_hlsl_source_dependencies(
            &root,
            "shaders/custom.hlsl",
            "\u{feff}# include <header.hlsli>\n",
        )
        .unwrap();
        assert_eq!(dependencies, vec!["assets/gpu/header.hlsli"]);
        fs::write(
            root.join("shaders/filter.hlsl"),
            "#include <header.hlsli>\n",
        )
        .unwrap();
        assert_eq!(
            collect_hlsl_dependencies(&root, "shaders/filter.hlsl").unwrap(),
            vec!["assets/gpu/header.hlsli", "shaders/filter.hlsl"]
        );
        fs::write(
            root.join("shaders/filter.hlsl"),
            "#include \"header.hlsli\"\n",
        )
        .unwrap();
        assert_eq!(
            collect_hlsl_dependencies(&root, "shaders/filter.hlsl").unwrap(),
            vec!["shaders/filter.hlsl", "shaders/header.hlsli"]
        );
        assert!(
            collect_hlsl_source_dependencies(
                &root,
                "custom.hlsl",
                "#include \"C:/outside.hlsl\"\n"
            )
            .is_err()
        );
        crate::history::remove_test_directory(&root);
    }
}
