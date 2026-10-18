use crate::graph::Pass;
use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct ShaderPerformance {
    pub dependency_walks: u64,
    pub root_resolutions: u64,
    pub include_reads: u64,
    pub include_bytes: u64,
    pub dxc_invocations: u64,
    pub spirv_cache_hits: u64,
}
static DEPENDENCY_WALKS: AtomicU64 = AtomicU64::new(0);
static ROOT_RESOLUTIONS: AtomicU64 = AtomicU64::new(0);
static INCLUDE_READS: AtomicU64 = AtomicU64::new(0);
static INCLUDE_BYTES: AtomicU64 = AtomicU64::new(0);
static DXC_INVOCATIONS: AtomicU64 = AtomicU64::new(0);
static SPIRV_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
/// Process-wide cumulative counters; callers can subtract snapshots around a
/// measured operation without resetting counters used by background workers.
pub fn shader_performance() -> ShaderPerformance {
    ShaderPerformance {
        dependency_walks: DEPENDENCY_WALKS.load(Ordering::Relaxed),
        root_resolutions: ROOT_RESOLUTIONS.load(Ordering::Relaxed),
        include_reads: INCLUDE_READS.load(Ordering::Relaxed),
        include_bytes: INCLUDE_BYTES.load(Ordering::Relaxed),
        dxc_invocations: DXC_INVOCATIONS.load(Ordering::Relaxed),
        spirv_cache_hits: SPIRV_CACHE_HITS.load(Ordering::Relaxed),
    }
}
/// Content revision of the selected graph's complete literal include closure.
/// Failed preparations use a rate-limited snapshot of this Result so fixing a
/// header (including recreating a missing file) can retry without a JSON edit.
pub fn shader_dependency_revision_at(
    graph: &module_engine_core::Graph,
    sources: &BTreeMap<String, String>,
    root: &Path,
) -> Result<u64, String> {
    let plan = crate::graph::build_plan_at(graph, sources, root)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for pass in plan.passes {
        pass.id.hash(&mut hash);
        pass.dependency_hash.hash(&mut hash);
    }
    Ok(hash.finish())
}
struct TemporaryShaderFiles {
    input: PathBuf,
    output: PathBuf,
}
impl Drop for TemporaryShaderFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.input);
        let _ = std::fs::remove_file(&self.output);
    }
}
#[derive(Clone, Debug)]
pub struct ShaderArtifact {
    pub words: Vec<u32>,
    pub diagnostics: String,
}
/// Assets/tool/cache root of a full workspace or a source-free team package.
/// Compiled shader sources remain embedded; a relocated package supplies DXC
/// and authored include files without consulting the author's source checkout.
pub fn resource_root() -> PathBuf {
    if let Some(root) = std::env::var_os("MODULE_ENGINE_RESOURCE_ROOT") {
        return root.into();
    }
    let executable = std::env::current_exe().ok();
    let mut candidates = vec![];
    // A distributed team executable owns its assets even when launched from
    // the author's full checkout. The Custom SDK is not a native source root.
    if let Some(root) = executable.as_ref().and_then(|path| path.parent())
        && root.join(".sdk").is_dir()
        && root.join(".module-engine-package.json").is_file()
    {
        candidates.push(root.to_path_buf());
    }
    candidates.extend(std::env::current_dir().ok());
    if let Some(executable) = executable
        && let Some(parent) = executable.parent()
    {
        candidates.push(parent.to_path_buf());
        if let Some(parent) = parent.parent() {
            candidates.push(parent.to_path_buf());
        }
    }
    candidates
        .into_iter()
        // Source-free Player packages need assets, not a compiler. Linux may
        // use a native DXC from PATH instead of any bundled Windows tools.
        .find(|root| root.join("assets/gpu").is_dir())
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}
pub fn dxc_path() -> PathBuf {
    let root = resource_root();
    resolve_dxc_at(
        &root,
        std::env::var_os("MODULE_ENGINE_DXC").as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
    .unwrap_or_else(|_| bundled_dxc_candidates(&root).remove(0))
}

fn bundled_dxc_candidates(root: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![root.join("tools/dxc/dxc.exe")]
    } else {
        vec![root.join("tools/dxc/bin/dxc"), root.join("tools/dxc/dxc")]
    }
}

fn resolve_dxc_at(
    root: &Path,
    explicit: Option<&std::ffi::OsStr>,
    search_path: Option<&std::ffi::OsStr>,
) -> Result<PathBuf, String> {
    let on_path = |name: &std::ffi::OsStr| {
        search_path
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|directory| directory.join(name))
            .find(|path| validate_native_dxc(path).is_ok())
    };
    if let Some(explicit) = explicit {
        let path = PathBuf::from(explicit);
        // Preserve Windows Command's existing bare-command override behavior.
        #[cfg(windows)]
        return Ok(path);
        #[cfg(not(windows))]
        {
            let resolved = if path.components().count() == 1 && !path.is_file() {
                on_path(explicit).unwrap_or(path)
            } else {
                path
            };
            validate_native_dxc(&resolved).map_err(|error| {
                format!("MODULE_ENGINE_DXC {}: {error}; set it to a native executable, not Windows dxc.exe", resolved.display())
            })?;
            return Ok(resolved);
        }
    }
    let candidates = bundled_dxc_candidates(root);
    if let Some(path) = candidates
        .iter()
        .find(|path| validate_native_dxc(path).is_ok())
    {
        return Ok(path.clone());
    }
    if let Some(path) = on_path(std::ffi::OsStr::new(if cfg!(windows) {
        "dxc.exe"
    } else {
        "dxc"
    })) {
        return Ok(path);
    }
    Err(format!(
        "No native DXC compiler for {}. Restore {} with its native shared libraries, install DXC yourself on PATH, or set MODULE_ENGINE_DXC. Precompiled Player shaders do not require DXC.",
        std::env::consts::OS,
        candidates[0].display()
    ))
}

fn validate_native_dxc(path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("not a regular file".into());
    }
    #[cfg(target_os = "linux")]
    {
        use std::{io::Read, os::unix::fs::PermissionsExt};
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("native Linux DXC is not executable (check file permissions)".into());
        }
        let mut magic = [0_u8; 4];
        std::fs::File::open(path)
            .and_then(|mut file| file.read_exact(&mut magic))
            .map_err(|error| error.to_string())?;
        if magic != *b"\x7fELF" {
            return Err("expected a native Linux ELF DXC executable".into());
        }
    }
    Ok(())
}

fn dxc_command(path: &Path) -> Result<Command, String> {
    let mut command = Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // Never open a compiler console.
    }
    #[cfg(target_os = "linux")]
    {
        // Native DXC release archives use bin/ and lib/. Restrict loader changes
        // to this child, preserving the caller's existing paths afterwards.
        let libraries = path.parent().and_then(|parent| {
            // Release layout: bin/dxc + lib/. Also support the documented
            // flat fallback dxc + lib/, without accidentally selecting tools/lib.
            let root = if parent.file_name() == Some(std::ffi::OsStr::new("bin")) {
                parent.parent()?
            } else {
                parent
            };
            Some(root.join("lib"))
        });
        if let Some(libraries) = libraries.filter(|path| path.is_dir()) {
            let mut paths = vec![libraries];
            if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            command.env(
                "LD_LIBRARY_PATH",
                std::env::join_paths(paths)
                    .map_err(|error| format!("DXC library path: {error}"))?,
            );
        }
    }
    Ok(command)
}
/// Fingerprint literal includes conservatively across all conditional branches.
/// The same content walk is used before compiling and when selecting a pipeline,
/// so editing a nested include invalidates cached SPIR-V/pipelines without DXC
/// work on every unchanged frame. Macro names have no complete dependency set.
pub(crate) fn dependency_fingerprint_at(root: &Path, source: &str) -> Result<u64, String> {
    DEPENDENCY_WALKS.fetch_add(1, Ordering::Relaxed);
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    shader_root_text(source).as_ref().hash(&mut hash);
    // Builtins and most inline shaders have no includes. Their source is the
    // entire dependency set, so even explicit prepare needs no filesystem IO.
    if literal_includes(source)?.is_empty() {
        return Ok(hash.finish());
    }
    ROOT_RESOLUTIONS.fetch_add(1, Ordering::Relaxed);
    let root = root
        .canonicalize()
        .map_err(|e| format!("HLSL workspace: {e}"))?;
    let mut seen = BTreeSet::new();
    let mut pending = vec![(None, source.to_owned())];
    while let Some((parent, text)) = pending.pop() {
        for (include, quoted) in literal_includes(&text)? {
            if include.starts_with('/') || include.contains(':') {
                return Err(format!("HLSL absolute include is forbidden: {include}"));
            }
            // Inline HLSL has no authored file directory: its documented roots
            // are assets/gpu then workspace. Nested files resolve their local
            // includes first, matching DXC's containing-file path behavior.
            let file = [
                parent.as_deref().filter(|_| quoted),
                Some(root.join("assets/gpu").as_path()),
                Some(root.as_path()),
            ]
            .into_iter()
            .flatten()
            .map(|directory| directory.join(&include))
            .filter_map(|candidate| candidate.canonicalize().ok())
            .find(|candidate| candidate.starts_with(&root) && candidate.is_file())
            .ok_or_else(|| format!("HLSL missing or out-of-project include: {include}"))?;
            if !seen.insert(file.clone()) {
                continue;
            }
            let bytes = std::fs::read(&file)
                .map_err(|e| format!("HLSL include {}: {e}", file.display()))?;
            INCLUDE_READS.fetch_add(1, Ordering::Relaxed);
            INCLUDE_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            file.strip_prefix(&root).unwrap().hash(&mut hash);
            bytes.hash(&mut hash);
            let text = String::from_utf8(bytes)
                .map_err(|e| format!("HLSL include {} must be UTF-8: {e}", file.display()))?;
            pending.push((file.parent().map(Path::to_path_buf), text));
        }
    }
    Ok(hash.finish())
}
fn literal_includes(source: &str) -> Result<Vec<(String, bool)>, String> {
    let text = without_comments(source.trim_start_matches('\u{feff}'));
    let mut includes = vec![];
    for line in text.lines() {
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
                "HLSL macro include has no stable dependency contract: {rest}"
            ));
        };
        let end = rest[1..].find(close).ok_or("HLSL malformed #include")? + 1;
        let include = rest[1..end].replace('\\', "/");
        if include.is_empty() {
            return Err("HLSL empty #include".into());
        }
        includes.push((include, close == '"'));
    }
    Ok(includes)
}
fn without_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let (mut index, mut block, mut quote) = (0, false, false);
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if block {
            if character == '*' && next == Some('/') {
                block = false;
                index += 2;
                out.push(' ');
                continue;
            }
            if character == '\n' {
                out.push(character);
            }
            index += 1;
            continue;
        }
        if character == '"' && (index == 0 || chars[index - 1] != '\\') {
            quote = !quote;
            out.push(character);
            index += 1;
            continue;
        }
        if !quote && character == '/' && next == Some('*') {
            block = true;
            index += 2;
            out.push(' ');
            continue;
        }
        if !quote && character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        out.push(character);
        index += 1;
    }
    out
}
pub fn compile_hlsl(source: &str, entry: &str, profile: &str) -> Result<ShaderArtifact, String> {
    compile_hlsl_at(&resource_root(), source, entry, profile)
}
/// Compile against an explicitly selected package, including its local includes.
pub fn compile_hlsl_at(
    root: &Path,
    source: &str,
    entry: &str,
    profile: &str,
) -> Result<ShaderArtifact, String> {
    let key = shader_key(root, source, entry, profile)?;
    let sealed = root.join(".spirv");
    let sealed_file = sealed.join(format!("{key:016x}.spv"));
    if sealed.is_dir() {
        let bytes = std::fs::read(&sealed_file).map_err(|e| {
            format!(
                "Precompiled Player shader {}: {e}; re-export this graph",
                sealed_file.display()
            )
        })?;
        SPIRV_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
        return artifact(bytes, "Precompiled Player SPIR-V".into());
    }
    let cache = root.join(".work/shader-cache");
    std::fs::create_dir_all(&cache).map_err(|e| format!("shader cache: {e}"))?;
    let stable = cache.join(format!("{key:016x}.spv"));
    if let Ok(bytes) = std::fs::read(&stable)
        && let Ok(shader) = artifact(bytes, "Cached SPIR-V".into())
    {
        SPIRV_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
        return Ok(shader);
    }
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = format!(
        "{key:016x}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let input = cache.join(format!("{name}.hlsl"));
    let output = cache.join(format!("{name}.spv"));
    let _temporary = TemporaryShaderFiles {
        input: input.clone(),
        output: output.clone(),
    };
    std::fs::write(&input, source).map_err(|e| e.to_string())?;
    let dxc = resolve_dxc_at(
        root,
        std::env::var_os("MODULE_ENGINE_DXC").as_deref(),
        std::env::var_os("PATH").as_deref(),
    )?;
    DXC_INVOCATIONS.fetch_add(1, Ordering::Relaxed);
    let result = dxc_command(&dxc)?
        .args([
            "-spirv",
            "-fspv-target-env=vulkan1.3",
            "-HV",
            "2021",
            "-T",
            profile,
            "-E",
            entry,
            "-Fo",
        ])
        .arg(&output)
        .arg("-I")
        .arg(root.join("assets/gpu"))
        .arg("-I")
        .arg(root)
        .arg(&input)
        .output()
        .map_err(|e| {
            format!(
                "Native DXC {}: {e}; restore this platform's tools/dxc and shared libraries or set MODULE_ENGINE_DXC",
                dxc.display()
            )
        })?;
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    if !result.status.success() {
        return Err(format!(
            "HLSL compilation failed ({profile}, entry {entry}):\n{diagnostics}"
        ));
    }
    let bytes = std::fs::read(&output).map_err(|e| e.to_string())?;
    let result = artifact(bytes.clone(), diagnostics)?;
    if shader_key(root, source, entry, profile)? != key {
        return Err("HLSL dependencies changed during compilation; retry the current files".into());
    }
    // Publish a complete artifact, avoiding partial reads by parallel compilation.
    if let Err(error) = std::fs::rename(&output, &stable) {
        if std::fs::read(&stable)
            .ok()
            .and_then(|b| artifact(b, String::new()).ok())
            .is_none()
        {
            return Err(format!("Publish shader cache: {error}"));
        }
        let _ = std::fs::remove_file(output);
    }
    let _ = std::fs::remove_file(input);
    Ok(result)
}
// Embedded and authored root text may carry different Windows line endings.
// Includes still contribute their exact bytes to the dependency fingerprint.
fn shader_root_text(source: &str) -> std::borrow::Cow<'_, str> {
    if source.contains("\r\n") {
        std::borrow::Cow::Owned(source.replace("\r\n", "\n"))
    } else {
        std::borrow::Cow::Borrowed(source)
    }
}
fn shader_key(root: &Path, source: &str, entry: &str, profile: &str) -> Result<u64, String> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    shader_root_text(source).as_ref().hash(&mut hash);
    dependency_fingerprint_at(root, source)?.hash(&mut hash);
    entry.hash(&mut hash);
    profile.hash(&mut hash);
    Ok(hash.finish())
}
fn artifact(bytes: Vec<u8>, diagnostics: String) -> Result<ShaderArtifact, String> {
    if !bytes.len().is_multiple_of(4) {
        return Err("DXC emitted malformed SPIR-V".into());
    }
    let words: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect();
    if words.len() < 5 || words.first() != Some(&0x07230203) {
        return Err("DXC output is not SPIR-V".into());
    }
    Ok(ShaderArtifact { words, diagnostics })
}
/// Compile every selected graph shader during export. A package with `.spirv`
/// uses only these files: it never starts DXC or falls back to author tools.
pub fn precompile_graph_at(
    graph: &module_engine_core::Graph,
    sources: &BTreeMap<String, String>,
    root: &Path,
    destination: &Path,
) -> Result<usize, String> {
    let plan = crate::graph::build_plan_at(graph, sources, root)?;
    let mut jobs = BTreeMap::new();
    for pass in &plan.passes {
        let profile = if pass.manifest.stage == "compute" {
            "cs_6_0"
        } else {
            "ps_6_0"
        };
        let shader = compile_hlsl_at(root, &pass.source, &pass.manifest.entry, profile)?;
        verify_shader(pass, &shader.words, false)?;
        jobs.insert(
            shader_key(root, &pass.source, &pass.manifest.entry, profile)?,
            shader.words,
        );
        if pass.manifest.stage != "compute" {
            let source = if pass.manifest.vertex_entry.is_some() {
                pass.source.as_str()
            } else if pass.geometry() {
                include_str!("../../../assets/gpu/forward.hlsl")
            } else {
                include_str!("../../../assets/gpu/fullscreen.hlsl")
            };
            let entry = pass.manifest.vertex_entry.as_deref().unwrap_or("mainVS");
            let shader = compile_hlsl_at(root, source, entry, "vs_6_0")?;
            verify_shader(pass, &shader.words, true)?;
            jobs.insert(shader_key(root, source, entry, "vs_6_0")?, shader.words);
        }
    }
    let output = destination.join(".spirv");
    std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    for (key, words) in &jobs {
        let bytes = words
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect::<Vec<_>>();
        std::fs::write(output.join(format!("{key:016x}.spv")), bytes).map_err(|e| e.to_string())?;
    }
    Ok(jobs.len())
}
pub(crate) fn verify_shader(pass: &Pass, words: &[u32], vertex: bool) -> Result<(), String> {
    let mut bindings = BTreeMap::new();
    let mut sets = BTreeMap::new();
    let mut pointer_types = BTreeMap::new();
    let mut variables = BTreeMap::new();
    let mut image_types = BTreeMap::new();
    let mut locals = None;
    let mut float_types = std::collections::BTreeSet::new();
    let mut index = 5;
    while index < words.len() {
        let instruction = words[index];
        let count = (instruction >> 16) as usize;
        let opcode = instruction & 0xffff;
        if count == 0 || index + count > words.len() {
            return Err("Malformed SPIR-V instruction".into());
        }
        let a = &words[index + 1..index + count];
        match opcode {
            16 if a.len() >= 5 && a[1] == 17 => locals = Some([a[2], a[3], a[4]]),
            71 if a.len() >= 3 && a[1] == 33 => {
                bindings.insert(a[0], a[2]);
            }
            71 if a.len() >= 3 && a[1] == 34 => {
                sets.insert(a[0], a[2]);
            }
            32 if a.len() >= 3 => {
                pointer_types.insert(a[0], a[2]);
            }
            59 if a.len() >= 2 => {
                variables.insert(a[1], a[0]);
            }
            22 if a.len() >= 2 && a[1] == 32 => {
                float_types.insert(a[0]);
            }
            25 if a.len() >= 8 => {
                image_types.insert(a[0], (a[6], a[7], a[1], a[2], a[4], a[5]));
            }
            _ => {}
        }
        index += count;
    }
    if pass.manifest.stage == "compute" && !vertex && locals != Some(pass.manifest.workgroup) {
        return Err(format!(
            "Custom {} workgroup manifest {:?} differs from compiled shader {:?}",
            pass.id, pass.manifest.workgroup, locals
        ));
    }
    let images: Vec<_> = pass
        .manifest
        .inputs
        .iter()
        .filter(|s| s.format != "scene")
        .collect();
    let material_input = usize::from(pass.manifest.scene_materials);
    let expected = images.len()
        + material_input
        + if pass.manifest.stage == "compute" && !vertex {
            pass.manifest.outputs.len()
        } else {
            0
        };
    for (id, binding) in &bindings {
        if sets.get(id) != Some(&0) || *binding as usize >= expected {
            return Err(format!(
                "Pass {} shader binding set={:?}, binding={} outside declared resources",
                pass.id,
                sets.get(id),
                binding
            ));
        }
        let type_id = variables
            .get(id)
            .and_then(|v| pointer_types.get(v))
            .ok_or("Shader resource is not a declared image")?;
        let (sampled, format, component, dimension, arrayed, multisampled) = image_types
            .get(type_id)
            .ok_or("Only typed texture resources are supported by the 0.2 Custom manifest")?;
        if !float_types.contains(component)
            || *dimension != 1
            || *arrayed != 0
            || *multisampled != 0
        {
            return Err(
                "Manifest resources require floating-point, 2D, single-layer, single-sample images"
                    .into(),
            );
        }
        let output = *binding as usize >= images.len() + material_input;
        if *sampled != if output { 2 } else { 1 } {
            return Err(
                "Compiled HLSL sampled/storage image type conflicts with declared access".into(),
            );
        }
        if output {
            let spec = &pass.manifest.outputs[*binding as usize - images.len()];
            let expected_format = match spec.format.as_str() {
                "rgba32float" => 1,
                "rgba8unorm" => 4,
                "r32float" => 3,
                _ => return Err("Unsupported SPIR-V image format".into()),
            };
            if *format != expected_format {
                return Err(format!(
                    "Output {} image format decoration {} differs from manifest {}; use [[vk::image_format(...)]]",
                    spec.name, format, spec.format
                ));
            }
        }
    }
    if pass.manifest.stage == "compute" && !vertex {
        for binding in images.len()..expected {
            if !bindings.values().any(|b| *b == binding as u32) {
                return Err(format!(
                    "Pass {} declared output binding {binding} is not used by compiled shader",
                    pass.id
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod dependency_tests {
    use super::*;

    fn eol_fixture(label: &str) -> PathBuf {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        let root = parent.join(format!(
            "module-hlsl-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn root_eol_identity_preserves_content_entry_profile_and_lone_cr() {
        let absent = std::env::temp_dir().join("module-hlsl-eol-no-root-needed");
        let lf = "// root comment\n#define GAIN (1 + \\\n2)\nfloat gain = GAIN;\n";
        let crlf = lf.replace('\n', "\r\n");
        let mixed = lf.replacen('\n', "\r\n", 1);
        let fingerprint = dependency_fingerprint_at(&absent, lf).unwrap();
        let key = shader_key(&absent, lf, "main", "cs_6_0").unwrap();
        for source in [crlf.as_str(), mixed.as_str()] {
            assert_eq!(
                dependency_fingerprint_at(&absent, source).unwrap(),
                fingerprint
            );
            assert_eq!(shader_key(&absent, source, "main", "cs_6_0").unwrap(), key);
        }
        for source in [lf.replace("1 +", "3 +"), lf.replacen('\n', "\r", 1)] {
            assert_ne!(
                dependency_fingerprint_at(&absent, &source).unwrap(),
                fingerprint
            );
            assert_ne!(shader_key(&absent, &source, "main", "cs_6_0").unwrap(), key);
        }
        assert_ne!(shader_key(&absent, lf, "other", "cs_6_0").unwrap(), key);
        assert_ne!(shader_key(&absent, lf, "main", "ps_6_0").unwrap(), key);
    }

    #[test]
    fn root_eol_identity_keeps_nested_include_bytes_and_old_mtime_changes_exact() {
        let root = eol_fixture("includes");
        std::fs::create_dir_all(root.join("assets/gpu/sub")).unwrap();
        std::fs::write(
            root.join("assets/gpu/shared.hlsli"),
            "#include \"sub/gain.hlsli\"\n",
        )
        .unwrap();
        let include = root.join("assets/gpu/sub/gain.hlsli");
        std::fs::write(&include, "float gain = 0.2;\n").unwrap();
        let before_time = std::fs::metadata(&include).unwrap().modified().unwrap();
        let lf = "// root\n#include \"shared.hlsli\"\n";
        let first = dependency_fingerprint_at(&root, lf).unwrap();
        let key = shader_key(&root, lf, "main", "cs_6_0").unwrap();
        assert_eq!(
            dependency_fingerprint_at(&root, &lf.replace('\n', "\r\n")).unwrap(),
            first
        );
        assert_eq!(
            shader_key(&root, &lf.replace('\n', "\r\n"), "main", "cs_6_0").unwrap(),
            key
        );
        std::fs::write(&include, "float gain = 0.8;\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&include)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(before_time))
            .unwrap();
        assert_eq!(
            std::fs::metadata(&include).unwrap().modified().unwrap(),
            before_time
        );
        assert_ne!(dependency_fingerprint_at(&root, lf).unwrap(), first);
        assert_ne!(shader_key(&root, lf, "main", "cs_6_0").unwrap(), key);
        std::fs::write(&include, "float gain = 0.2;\r\n").unwrap();
        assert_ne!(
            dependency_fingerprint_at(&root, lf).unwrap(),
            first,
            "Included EOL bytes remain exact"
        );
        assert_ne!(shader_key(&root, lf, "main", "cs_6_0").unwrap(), key);
        std::fs::write(&include, "float gain = 0.2;\n").unwrap();
        assert_eq!(dependency_fingerprint_at(&root, lf).unwrap(), first);
        assert_eq!(shader_key(&root, lf, "main", "cs_6_0").unwrap(), key);
        assert_eq!(root.canonicalize().unwrap(), root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn genuine_dxc_shader_sealed_lookup_accepts_root_eol_only_without_fallback() {
        let package = eol_fixture("sealed");
        let source = format!(
            "// unique fixture {}\n[[vk::binding(0,0)]] RWStructuredBuffer<uint> result;\n#define VALUE (1u + \\\n2u)\n[numthreads(1,1,1)] void main(uint3 id : SV_DispatchThreadID) {{ result[0] = VALUE; }}\n",
            package.file_name().unwrap().to_string_lossy()
        );
        let compiler_root = resource_root();
        let compiled = compile_hlsl_at(&compiler_root, &source, "main", "cs_6_0").unwrap();
        assert_ne!(compiled.diagnostics, "Cached SPIR-V");
        assert_ne!(compiled.diagnostics, "Precompiled Player SPIR-V");
        let key = shader_key(&package, &source, "main", "cs_6_0").unwrap();
        let sealed = package.join(".spirv");
        std::fs::create_dir(&sealed).unwrap();
        let artifact = sealed.join(format!("{key:016x}.spv"));
        let bytes: Vec<_> = compiled
            .words
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect();
        std::fs::write(&artifact, &bytes).unwrap();
        let dxc_before = shader_performance().dxc_invocations;
        for source in [
            &source,
            &source.replace('\n', "\r\n"),
            &source.replacen('\n', "\r\n", 1),
        ] {
            let loaded = compile_hlsl_at(&package, source, "main", "cs_6_0").unwrap();
            assert_eq!(loaded.words, compiled.words);
            assert_eq!(loaded.diagnostics, "Precompiled Player SPIR-V");
        }
        let error = compile_hlsl_at(&package, &source.replace("1u +", "3u +"), "main", "cs_6_0")
            .unwrap_err();
        assert!(
            error.contains("Precompiled Player shader") && error.contains("re-export"),
            "{error}"
        );
        assert_eq!(std::fs::read(&artifact).unwrap(), bytes);
        std::fs::remove_file(&artifact).unwrap();
        assert!(
            compile_hlsl_at(&package, &source, "main", "cs_6_0")
                .unwrap_err()
                .contains("re-export")
        );
        // Like the native GPU checks, run this global-counter delta serially.
        assert_eq!(shader_performance().dxc_invocations, dxc_before);
        assert!(
            !package.join(".work").exists(),
            "A sealed lookup must not enter compiler/cache fallback"
        );
        assert_eq!(package.canonicalize().unwrap(), package);
        std::fs::remove_dir_all(package).unwrap();
    }

    #[test]
    fn native_dxc_discovery_is_deterministic_and_missing_tool_is_actionable() {
        let root = std::env::temp_dir().join(format!(
            "module-engine-dxc-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let error = resolve_dxc_at(&root, None, None).unwrap_err();
        assert!(error.contains("No native DXC compiler"));
        assert!(error.contains("MODULE_ENGINE_DXC"));
        assert!(error.contains("Precompiled Player shaders do not require DXC"));
        let candidate = bundled_dxc_candidates(&root).remove(0);
        assert_eq!(
            candidate.file_name().unwrap(),
            if cfg!(windows) { "dxc.exe" } else { "dxc" }
        );
        std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
        // Selection fixture only, not a compiler invocation or Linux runtime proof.
        std::fs::write(&candidate, b"\x7fELFfixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(resolve_dxc_at(&root, None, None).unwrap(), candidate);
        let explicit = root.join(if cfg!(windows) {
            "explicit.exe"
        } else {
            "explicit"
        });
        std::fs::copy(&candidate, &explicit).unwrap();
        assert_eq!(
            resolve_dxc_at(&root, Some(explicit.as_os_str()), None).unwrap(),
            explicit
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_dxc_rejects_pe_and_non_executable_files_and_uses_child_local_libraries() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("module-engine-dxc-linux-{}", std::process::id()));
        let compiler = root.join("tools/dxc/bin/dxc");
        std::fs::create_dir_all(compiler.parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.join("tools/dxc/lib")).unwrap();
        std::fs::write(&compiler, b"MZWindowsExecutable").unwrap();
        std::fs::set_permissions(&compiler, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(validate_native_dxc(&compiler).unwrap_err().contains("ELF"));
        let error = resolve_dxc_at(&root, Some(compiler.as_os_str()), None).unwrap_err();
        assert!(error.contains("MODULE_ENGINE_DXC"));
        assert!(error.contains("not Windows dxc.exe"));
        std::fs::write(&compiler, b"\x7fELFfixture").unwrap();
        std::fs::set_permissions(&compiler, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            validate_native_dxc(&compiler)
                .unwrap_err()
                .contains("not executable")
        );
        std::fs::set_permissions(&compiler, std::fs::Permissions::from_mode(0o755)).unwrap();
        let flat_compiler = root.join("tools/dxc/dxc");
        std::fs::copy(&compiler, &flat_compiler).unwrap();
        for native_compiler in [&compiler, &flat_compiler] {
            let command = dxc_command(native_compiler).unwrap();
            let library_path = command
                .get_envs()
                .find(|(key, _)| *key == "LD_LIBRARY_PATH")
                .unwrap()
                .1
                .unwrap();
            assert_eq!(
                std::env::split_paths(library_path).next().unwrap(),
                root.join("tools/dxc/lib"),
                "native library path for {}",
                native_compiler.display()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_without_includes_needs_no_filesystem_root() {
        let absent = std::env::temp_dir().join("module-engine-no-root-needed-for-inline-hlsl");
        let first =
            dependency_fingerprint_at(&absent, "// #include BAD\nfloat gain = 0.2;").unwrap();
        assert_ne!(
            first,
            dependency_fingerprint_at(&absent, "float gain = 0.8;").unwrap()
        );
        assert!(
            dependency_fingerprint_at(&absent, "#include MISSING")
                .unwrap_err()
                .contains("macro include")
        );
        assert!(dependency_fingerprint_at(&absent, "#include \"missing.hlsli\"").is_err());
    }
    #[test]
    fn recursive_content_roots_comments_cycles_and_angle_resolution() {
        let root =
            std::env::temp_dir().join(format!("module-engine-hlsl-roots-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets/gpu/sub")).unwrap();
        std::fs::write(
            root.join("assets/gpu/shared.hlsli"),
            "#include \"sub/weight.hlsli\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("assets/gpu/sub/weight.hlsli"),
            "#include \"../shared.hlsli\"\nfloat gain = 0.2;\n",
        )
        .unwrap();
        std::fs::write(root.join("shared.hlsli"), "float shadowed = 99.;\n").unwrap();
        let source =
            "\u{feff}/* #include MISSING */\n// #include \"absent\"\n# include <shared.hlsli>\n";
        let first = dependency_fingerprint_at(&root, source).unwrap();
        std::fs::write(root.join("shared.hlsli"), "float shadowed = 12.;\n").unwrap();
        assert_eq!(
            dependency_fingerprint_at(&root, source).unwrap(),
            first,
            "DXC assets/gpu root must take priority"
        );
        std::fs::write(
            root.join("assets/gpu/sub/weight.hlsli"),
            "#include \"../shared.hlsli\"\nfloat gain = 0.8;\n",
        )
        .unwrap();
        assert_ne!(
            dependency_fingerprint_at(&root, source).unwrap(),
            first,
            "Nested bytes must affect fingerprint despite cycle"
        );
        std::fs::write(
            root.join("assets/gpu/shared.hlsli"),
            "#include <sub/weight.hlsli>\n",
        )
        .unwrap();
        assert!(
            dependency_fingerprint_at(&root, source).is_ok(),
            "Angle include uses explicit assets/gpu root"
        );
        std::fs::write(
            root.join("assets/gpu/sub/local.hlsli"),
            "float local_value = 1.;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("assets/gpu/sub/weight.hlsli"),
            "#include <local.hlsli>\n",
        )
        .unwrap();
        assert!(
            dependency_fingerprint_at(&root, source)
                .unwrap_err()
                .contains("missing"),
            "Nested angle include must not accidentally search local parent"
        );
        for unsupported in [
            "#include HEADER\n",
            "#include \"absent.hlsli\"\n",
            "#include \"C:/outside.hlsli\"\n",
            "#include \"../../outside.hlsli\"\n",
        ] {
            assert!(
                dependency_fingerprint_at(&root, unsupported).is_err(),
                "Dependency gaps require an explicit error: {unsupported}"
            );
        }
    }
}
