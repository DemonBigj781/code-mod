use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use camoufox::builder::{HeadlessMode, LaunchOptions, PreparedLaunch};
use camoufox::prepare;
use camoufox_core::config::WINDOW_DIM_KEYS;
use camoufox_core::os::SupportedOs;
use camoufox_pkgman::DefaultAddon;
use fs2::FileExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::ZipArchive;

const REQUIRED_RUNTIME_FILES: &[&str] = &[
    "manifest.json",
    "camoufox/camoufox-bin",
    "camoufox/camoufox.cfg",
    "camoufox/camoucfg.jvv",
    "camoufox/properties.json",
    "camoufox/GeoLite2-City.mmdb",
    "lib/libEGL.so.1",
    "lib/libGL.so.1",
    "lib/libGLX_mesa.so.0",
    "lib/libLLVM.so",
    "lib/dri/swrast_dri.so",
    "licenses/camoufox-MPL-2.0.txt",
    "licenses/mesa-MIT.txt",
    "licenses/llvm-Apache-2.0-WITH-LLVM-exception.txt",
];

const CAMOU_CONFIG_PREFIX: &str = "CAMOU_CONFIG_";
const EMBEDDED_RUNTIME_PREFIX: &str = "browser-runtime/";
const RUNTIME_MANIFEST_SCHEMA: u32 = 2;
const RUNTIME_TARGET: &str = "x86_64-unknown-linux-gnu";
const CAMOUFOX_SOURCE_REVISION: &str = "0583c3ec94f5a9df5cb2d09553fbfe80589b6e2d";
const CAMOUFOX_RUST_REVISION: &str = "933ae2e8ba7a00e254b4d4075eb0e85a527289fc";
const EXTRACTION_LOCK_WAIT: Duration = Duration::from_secs(120);
const EXTRACTION_LOCK_STALE: Duration = Duration::from_secs(10 * 60);
const RUNTIME_IO_BUFFER_SIZE: usize = 1024 * 1024;
const PROFILE_LEASE_FILE: &str = "profile.mcp.lock";
const FIREFOX_PROFILE_LOCKS: &[&str] = &["lock", ".parentlock", "parent.lock"];

#[derive(Debug)]
pub(crate) struct BrowserRuntime {
    root: PathBuf,
    executable: PathBuf,
    firefox_major: String,
}

pub(crate) struct PreparedBrowserLaunch {
    pub(crate) options: LaunchOptions,
    pub(crate) prepared: PreparedLaunch,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifest {
    schema_version: u32,
    target: String,
    archive_prefix: String,
    camoufox: CamoufoxManifest,
    automation: AutomationManifest,
    firefox: FirefoxManifest,
    tree_sha256: String,
    files: Vec<RuntimeFile>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CamoufoxManifest {
    source_revision: String,
    rust_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationManifest {
    protocol: String,
    driver: String,
    driver_revision: String,
    entrypoint: String,
    transport: String,
}

#[derive(Debug, Deserialize)]
struct FirefoxManifest {
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeFile {
    path: String,
    #[serde(rename = "type")]
    kind: RuntimeFileKind,
    target: Option<String>,
    size: u64,
    sha256: String,
    mode: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RuntimeFileKind {
    File,
    Symlink,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserRuntimeErrorKind {
    Other,
    UniformDarkFrame,
}

#[derive(Debug)]
pub(crate) struct BrowserRuntimeError {
    kind: BrowserRuntimeErrorKind,
    message: String,
}

impl BrowserRuntimeError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            kind: BrowserRuntimeErrorKind::Other,
            message: message.into(),
        }
    }

    pub(crate) fn uniform_dark_frame(dominance: f64, color: [u8; 3]) -> Self {
        Self {
            kind: BrowserRuntimeErrorKind::UniformDarkFrame,
            message: format!(
                "browser screenshot is an overwhelmingly uniform dark frame ({:.2}% rgb({}, {}, {})); the Camoufox/Mesa renderer did not produce page content",
                dominance * 100.0,
                color[0],
                color[1],
                color[2]
            ),
        }
    }

    pub(crate) fn is_uniform_dark_frame(&self) -> bool {
        self.kind == BrowserRuntimeErrorKind::UniformDarkFrame
    }
}

impl fmt::Display for BrowserRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BrowserRuntimeError {}

impl BrowserRuntime {
    pub(crate) fn discover() -> Result<Self, BrowserRuntimeError> {
        if let Some(root) = env::var_os("CODE_BROWSER_RUNTIME_DIR") {
            return Self::open(PathBuf::from(root));
        }
        let root = code_home().join("browser/runtime");
        if let Ok(runtime) = Self::open(&root) {
            return Ok(runtime);
        }
        install_embedded_runtime(&root)?;
        Self::open(root)
    }

    pub(crate) fn open(root: impl AsRef<Path>) -> Result<Self, BrowserRuntimeError> {
        let root = root.as_ref();
        let mut missing = Vec::new();
        for relative in REQUIRED_RUNTIME_FILES {
            let path = root.join(relative);
            if !path.is_file() {
                missing.push((*relative).to_owned());
            }
        }
        if !missing.is_empty() {
            return Err(BrowserRuntimeError::new(format!(
                "Camoufox browser runtime at {} is incomplete; missing: {}",
                root.display(),
                missing.join(", ")
            )));
        }

        let manifest = read_manifest(root)?;
        validate_manifest(root, &manifest)?;
        let firefox_major = manifest
            .firefox
            .version
            .split('.')
            .next()
            .filter(|major| !major.is_empty() && major.bytes().all(|byte| byte.is_ascii_digit()))
            .ok_or_else(|| {
                BrowserRuntimeError::new(format!(
                    "Camoufox runtime manifest has an invalid Firefox version: {}",
                    manifest.firefox.version
                ))
            })?
            .to_owned();

        let executable = root.join("camoufox/camoufox-bin");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&executable)
                .map_err(|error| {
                    BrowserRuntimeError::new(format!(
                        "could not inspect Camoufox executable {}: {error}",
                        executable.display()
                    ))
                })?
                .permissions()
                .mode();
            if mode & 0o111 == 0 {
                return Err(BrowserRuntimeError::new(format!(
                    "Camoufox executable is not executable: {}",
                    executable.display()
                )));
            }
        }

        Ok(Self {
            root: root.to_path_buf(),
            executable,
            firefox_major,
        })
    }

    pub(crate) fn executable(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn child_environment(&self) -> BTreeMap<String, String> {
        let library_dir = self.root.join("lib");
        let browser_dir = self.root.join("camoufox");
        let mut library_path = vec![
            library_dir.to_string_lossy().into_owned(),
            browser_dir.to_string_lossy().into_owned(),
        ];
        if let Some(existing) = env::var_os("LD_LIBRARY_PATH")
            && !existing.is_empty()
        {
            library_path.push(existing.to_string_lossy().into_owned());
        }

        BTreeMap::from([
            ("MOZ_HEADLESS".to_owned(), "1".to_owned()),
            ("LIBGL_ALWAYS_SOFTWARE".to_owned(), "1".to_owned()),
            ("GALLIUM_DRIVER".to_owned(), "llvmpipe".to_owned()),
            (
                "MESA_LOADER_DRIVER_OVERRIDE".to_owned(),
                "llvmpipe".to_owned(),
            ),
            ("EGL_PLATFORM".to_owned(), "surfaceless".to_owned()),
            (
                "LIBGL_DRIVERS_PATH".to_owned(),
                self.root.join("lib/dri").to_string_lossy().into_owned(),
            ),
            (
                "FONTCONFIG_PATH".to_owned(),
                browser_dir
                    .join("fontconfig/linux")
                    .to_string_lossy()
                    .into_owned(),
            ),
            ("LD_LIBRARY_PATH".to_owned(), library_path.join(":")),
        ])
    }
}

fn read_manifest(root: &Path) -> Result<RuntimeManifest, BrowserRuntimeError> {
    let manifest_path = root.join("manifest.json");
    let bytes = fs::read(&manifest_path).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not read Camoufox runtime manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "invalid Camoufox runtime manifest {}: {error}",
            manifest_path.display()
        ))
    })
}

fn validate_manifest(root: &Path, manifest: &RuntimeManifest) -> Result<(), BrowserRuntimeError> {
    if manifest.schema_version != RUNTIME_MANIFEST_SCHEMA {
        return Err(BrowserRuntimeError::new(format!(
            "unsupported Camoufox runtime manifest schema {}; expected {RUNTIME_MANIFEST_SCHEMA}",
            manifest.schema_version
        )));
    }
    if manifest.target != RUNTIME_TARGET {
        return Err(BrowserRuntimeError::new(format!(
            "Camoufox runtime target {} is incompatible with {RUNTIME_TARGET}",
            manifest.target
        )));
    }
    if manifest.archive_prefix != EMBEDDED_RUNTIME_PREFIX {
        return Err(BrowserRuntimeError::new(format!(
            "Camoufox runtime archive prefix {:?} does not match {:?}",
            manifest.archive_prefix, EMBEDDED_RUNTIME_PREFIX
        )));
    }
    if manifest.camoufox.source_revision != CAMOUFOX_SOURCE_REVISION {
        return Err(BrowserRuntimeError::new(format!(
            "Camoufox runtime source revision {} does not match the pinned revision {CAMOUFOX_SOURCE_REVISION}",
            manifest.camoufox.source_revision
        )));
    }
    if manifest.camoufox.rust_revision != CAMOUFOX_RUST_REVISION {
        return Err(BrowserRuntimeError::new(format!(
            "Camoufox Rust runtime revision {} does not match the pinned revision {CAMOUFOX_RUST_REVISION}",
            manifest.camoufox.rust_revision
        )));
    }
    if manifest.automation.protocol != "juggler-pipe"
        || manifest.automation.driver != "camoufox-juggler"
        || manifest.automation.driver_revision != CAMOUFOX_RUST_REVISION
        || manifest.automation.entrypoint != "rustwright-mcp"
        || manifest.automation.transport != "mcp-stdio-json-rpc"
    {
        return Err(BrowserRuntimeError::new(
            "Camoufox runtime automation API contract is incomplete or incompatible",
        ));
    }
    if manifest.files.is_empty() {
        return Err(BrowserRuntimeError::new(
            "Camoufox runtime manifest contains no file inventory",
        ));
    }

    let mut paths = BTreeSet::new();
    let mut tree = Sha256::new();
    for file in &manifest.files {
        let relative = safe_relative_path(&file.path)?;
        if !paths.insert(file.path.as_str()) {
            return Err(BrowserRuntimeError::new(format!(
                "Camoufox runtime manifest repeats path {}",
                file.path
            )));
        }
        let mode = parse_mode(&file.mode)?;
        let path = root.join(relative);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            BrowserRuntimeError::new(format!(
                "Camoufox runtime file {} is unavailable: {error}",
                path.display()
            ))
        })?;
        let actual_hash = match file.kind {
            RuntimeFileKind::File => {
                if !metadata.file_type().is_file() {
                    return Err(BrowserRuntimeError::new(format!(
                        "Camoufox runtime path is not a regular file: {}",
                        path.display()
                    )));
                }
                if metadata.len() != file.size {
                    return Err(BrowserRuntimeError::new(format!(
                        "Camoufox runtime file size mismatch for {}: {} != {}",
                        path.display(),
                        metadata.len(),
                        file.size
                    )));
                }
                hash_file(&path)?
            }
            RuntimeFileKind::Symlink => {
                if !metadata.file_type().is_symlink() {
                    return Err(BrowserRuntimeError::new(format!(
                        "Camoufox runtime path is not a symlink: {}",
                        path.display()
                    )));
                }
                let target = fs::read_link(&path).map_err(|error| {
                    BrowserRuntimeError::new(format!(
                        "could not read Camoufox runtime symlink {}: {error}",
                        path.display()
                    ))
                })?;
                let target = target.to_str().ok_or_else(|| {
                    BrowserRuntimeError::new(format!(
                        "Camoufox runtime symlink target is not UTF-8: {}",
                        path.display()
                    ))
                })?;
                validate_symlink_target(&file.path, target)?;
                if file.target.as_deref() != Some(target) {
                    return Err(BrowserRuntimeError::new(format!(
                        "Camoufox runtime symlink target mismatch for {}",
                        path.display()
                    )));
                }
                if target.len() as u64 != file.size {
                    return Err(BrowserRuntimeError::new(format!(
                        "Camoufox runtime symlink size mismatch for {}",
                        path.display()
                    )));
                }
                hash_bytes(target.as_bytes())
            }
        };
        if actual_hash != file.sha256 {
            return Err(BrowserRuntimeError::new(format!(
                "Camoufox runtime SHA-256 mismatch for {}: {} != {}",
                path.display(),
                actual_hash,
                file.sha256
            )));
        }
        #[cfg(unix)]
        if file.kind == RuntimeFileKind::File {
            use std::os::unix::fs::PermissionsExt;
            let actual_mode = metadata.permissions().mode() & 0o7777;
            if actual_mode != mode {
                return Err(BrowserRuntimeError::new(format!(
                    "Camoufox runtime mode mismatch for {}: {actual_mode:04o} != {mode:04o}",
                    path.display()
                )));
            }
        }
        update_tree_digest(&mut tree, file);
    }
    for required in REQUIRED_RUNTIME_FILES {
        if *required != "manifest.json" && !paths.contains(required) {
            return Err(BrowserRuntimeError::new(format!(
                "Camoufox runtime manifest omits required path {required}"
            )));
        }
    }
    let actual_tree = hex_digest(tree.finalize());
    if actual_tree != manifest.tree_sha256 {
        return Err(BrowserRuntimeError::new(format!(
            "Camoufox runtime tree SHA-256 mismatch: {actual_tree} != {}",
            manifest.tree_sha256
        )));
    }
    Ok(())
}

fn safe_relative_path(raw: &str) -> Result<&Path, BrowserRuntimeError> {
    let path = Path::new(raw);
    if raw.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BrowserRuntimeError::new(format!(
            "unsafe Camoufox runtime path in manifest: {raw:?}"
        )));
    }
    Ok(path)
}

fn validate_symlink_target(path: &str, target: &str) -> Result<(), BrowserRuntimeError> {
    let target_path = Path::new(target);
    if target.is_empty() || target_path.is_absolute() {
        return Err(BrowserRuntimeError::new(format!(
            "unsafe Camoufox runtime symlink: {path} -> {target}"
        )));
    }
    let mut depth = Path::new(path).parent().map_or(0, |parent| {
        parent
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .count()
    });
    for component in target_path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(_) => depth += 1,
            Component::ParentDir if depth > 0 => depth -= 1,
            _ => {
                return Err(BrowserRuntimeError::new(format!(
                    "Camoufox runtime symlink escapes the payload: {path} -> {target}"
                )));
            }
        }
    }
    Ok(())
}

fn parse_mode(raw: &str) -> Result<u32, BrowserRuntimeError> {
    u32::from_str_radix(raw, 8).map_err(|error| {
        BrowserRuntimeError::new(format!("invalid Camoufox runtime mode {raw:?}: {error}"))
    })
}

fn hash_file(path: &Path) -> Result<String, BrowserRuntimeError> {
    let mut input = fs::File::open(path).map_err(|error| {
        BrowserRuntimeError::new(format!("could not hash {}: {error}", path.display()))
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; RUNTIME_IO_BUFFER_SIZE];
    loop {
        let count = input.read(&mut buffer).map_err(|error| {
            BrowserRuntimeError::new(format!("could not hash {}: {error}", path.display()))
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex_digest(digest.finalize()))
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex_digest(Sha256::digest(bytes))
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn update_tree_digest(digest: &mut Sha256, file: &RuntimeFile) {
    let kind = match file.kind {
        RuntimeFileKind::File => "file",
        RuntimeFileKind::Symlink => "symlink",
    };
    let size = file.size.to_string();
    for (index, field) in [
        kind,
        file.path.as_str(),
        file.mode.as_str(),
        size.as_str(),
        file.sha256.as_str(),
        file.target.as_deref().unwrap_or(""),
    ]
    .into_iter()
    .enumerate()
    {
        if index > 0 {
            digest.update([0]);
        }
        digest.update(field.as_bytes());
    }
    digest.update(b"\n");
}

struct ExtractionLock {
    path: PathBuf,
}

impl Drop for ExtractionLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn install_embedded_runtime(root: &Path) -> Result<(), BrowserRuntimeError> {
    let parent = root.parent().ok_or_else(|| {
        BrowserRuntimeError::new(format!(
            "Camoufox runtime path has no parent: {}",
            root.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not create Camoufox runtime parent {}: {error}",
            parent.display()
        ))
    })?;
    let _lock = acquire_extraction_lock(&parent.join("runtime.extract.lock"))?;
    if BrowserRuntime::open(root).is_ok() {
        return Ok(());
    }

    let executable = env::current_exe().map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not locate the Code executable for embedded browser extraction: {error}"
        ))
    })?;
    extract_embedded_runtime(&executable, root)
}

fn acquire_extraction_lock(path: &Path) -> Result<ExtractionLock, BrowserRuntimeError> {
    let deadline = SystemTime::now() + EXTRACTION_LOCK_WAIT;
    loop {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            Ok(mut file) => {
                writeln!(file, "{}", std::process::id()).map_err(|error| {
                    BrowserRuntimeError::new(format!(
                        "could not write Camoufox extraction lock {}: {error}",
                        path.display()
                    ))
                })?;
                file.sync_all().map_err(|error| {
                    BrowserRuntimeError::new(format!(
                        "could not sync Camoufox extraction lock {}: {error}",
                        path.display()
                    ))
                })?;
                return Ok(ExtractionLock {
                    path: path.to_path_buf(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .and_then(|modified| {
                        SystemTime::now()
                            .duration_since(modified)
                            .map_err(std::io::Error::other)
                    })
                    .is_ok_and(|age| age >= EXTRACTION_LOCK_STALE);
                if stale {
                    let _ = fs::remove_file(path);
                    continue;
                }
                if SystemTime::now() >= deadline {
                    return Err(BrowserRuntimeError::new(format!(
                        "timed out waiting for Camoufox runtime extraction lock {}",
                        path.display()
                    )));
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                return Err(BrowserRuntimeError::new(format!(
                    "could not acquire Camoufox runtime extraction lock {}: {error}",
                    path.display()
                )));
            }
        }
    }
}

fn extract_embedded_runtime(archive_path: &Path, root: &Path) -> Result<(), BrowserRuntimeError> {
    static NEXT_EXTRACTION: AtomicUsize = AtomicUsize::new(1);
    let input = fs::File::open(archive_path).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not open Code executable {} for browser extraction: {error}",
            archive_path.display()
        ))
    })?;
    let mut archive = ZipArchive::new(input).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "Code executable {} contains no embedded Camoufox runtime archive: {error}",
            archive_path.display()
        ))
    })?;
    let manifest_name = format!("{EMBEDDED_RUNTIME_PREFIX}manifest.json");
    let manifest_bytes = {
        let mut entry = archive.by_name(&manifest_name).map_err(|error| {
            BrowserRuntimeError::new(format!(
                "Code executable contains no embedded Camoufox manifest {manifest_name}: {error}"
            ))
        })?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| {
            BrowserRuntimeError::new(format!(
                "could not read embedded Camoufox manifest: {error}"
            ))
        })?;
        bytes
    };
    let manifest: RuntimeManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        BrowserRuntimeError::new(format!("embedded Camoufox manifest is invalid: {error}"))
    })?;
    validate_manifest_contract(&manifest)?;
    validate_archive_inventory(&mut archive, &manifest)?;

    let parent = root.parent().ok_or_else(|| {
        BrowserRuntimeError::new(format!("runtime path has no parent: {}", root.display()))
    })?;
    let temporary = parent.join(format!(
        ".runtime.extract.{}.{}",
        std::process::id(),
        NEXT_EXTRACTION.fetch_add(1, Ordering::Relaxed)
    ));
    if fs::symlink_metadata(&temporary).is_ok() {
        fs::remove_dir_all(&temporary).map_err(|error| {
            BrowserRuntimeError::new(format!(
                "could not clear stale extraction directory {}: {error}",
                temporary.display()
            ))
        })?;
    }
    fs::create_dir(&temporary).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not create Camoufox extraction directory {}: {error}",
            temporary.display()
        ))
    })?;

    let extraction = (|| {
        for file in &manifest.files {
            extract_runtime_file(&mut archive, &temporary, file)?;
        }
        fs::write(temporary.join("manifest.json"), &manifest_bytes).map_err(|error| {
            BrowserRuntimeError::new(format!(
                "could not write extracted Camoufox manifest: {error}"
            ))
        })?;
        BrowserRuntime::open(&temporary)?;
        replace_runtime_directory(root, &temporary)
    })();
    if extraction.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    extraction
}

fn validate_manifest_contract(manifest: &RuntimeManifest) -> Result<(), BrowserRuntimeError> {
    if manifest.schema_version != RUNTIME_MANIFEST_SCHEMA
        || manifest.target != RUNTIME_TARGET
        || manifest.archive_prefix != EMBEDDED_RUNTIME_PREFIX
        || manifest.camoufox.source_revision != CAMOUFOX_SOURCE_REVISION
        || manifest.camoufox.rust_revision != CAMOUFOX_RUST_REVISION
        || manifest.files.is_empty()
    {
        return Err(BrowserRuntimeError::new(
            "embedded Camoufox manifest does not match the pinned runtime contract",
        ));
    }
    Ok(())
}

fn validate_archive_inventory(
    archive: &mut ZipArchive<fs::File>,
    manifest: &RuntimeManifest,
) -> Result<(), BrowserRuntimeError> {
    let mut actual = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            BrowserRuntimeError::new(format!(
                "could not inspect embedded runtime archive: {error}"
            ))
        })?;
        let name = entry.name();
        if name.starts_with(EMBEDDED_RUNTIME_PREFIX) && !actual.insert(name.to_owned()) {
            return Err(BrowserRuntimeError::new(format!(
                "embedded runtime archive repeats entry {name}"
            )));
        }
    }
    let mut expected = BTreeSet::from([format!("{EMBEDDED_RUNTIME_PREFIX}manifest.json")]);
    for file in &manifest.files {
        safe_relative_path(&file.path)?;
        expected.insert(format!("{EMBEDDED_RUNTIME_PREFIX}{}", file.path));
    }
    if actual != expected {
        let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
        let extra = actual.difference(&expected).cloned().collect::<Vec<_>>();
        return Err(BrowserRuntimeError::new(format!(
            "embedded Camoufox archive inventory mismatch; missing={missing:?}, extra={extra:?}"
        )));
    }
    Ok(())
}

fn extract_runtime_file(
    archive: &mut ZipArchive<fs::File>,
    root: &Path,
    file: &RuntimeFile,
) -> Result<(), BrowserRuntimeError> {
    let relative = safe_relative_path(&file.path)?;
    let output = root.join(relative);
    let parent = output.parent().ok_or_else(|| {
        BrowserRuntimeError::new(format!("runtime file has no parent: {}", output.display()))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not create runtime directory {}: {error}",
            parent.display()
        ))
    })?;
    let name = format!("{EMBEDDED_RUNTIME_PREFIX}{}", file.path);
    let mut entry = archive.by_name(&name).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "embedded runtime entry {name} is unavailable: {error}"
        ))
    })?;
    let archive_mode = entry.unix_mode().unwrap_or(0) & 0o7777;
    let expected_mode = parse_mode(&file.mode)?;
    if archive_mode != expected_mode {
        return Err(BrowserRuntimeError::new(format!(
            "embedded runtime mode mismatch for {name}: {archive_mode:04o} != {expected_mode:04o}"
        )));
    }
    let entry_type_matches = match file.kind {
        RuntimeFileKind::File => entry.is_file(),
        RuntimeFileKind::Symlink => entry.is_symlink(),
    };
    if !entry_type_matches {
        return Err(BrowserRuntimeError::new(format!(
            "embedded runtime entry type mismatch for {name}"
        )));
    }
    match file.kind {
        RuntimeFileKind::File => {
            let mut output_file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|error| {
                    BrowserRuntimeError::new(format!(
                        "could not create extracted runtime file {}: {error}",
                        output.display()
                    ))
                })?;
            let mut digest = Sha256::new();
            let mut size = 0_u64;
            let mut buffer = vec![0_u8; RUNTIME_IO_BUFFER_SIZE];
            loop {
                let count = entry.read(&mut buffer).map_err(|error| {
                    BrowserRuntimeError::new(format!("could not read {name}: {error}"))
                })?;
                if count == 0 {
                    break;
                }
                output_file.write_all(&buffer[..count]).map_err(|error| {
                    BrowserRuntimeError::new(format!(
                        "could not write extracted runtime file {}: {error}",
                        output.display()
                    ))
                })?;
                digest.update(&buffer[..count]);
                size += count as u64;
            }
            if size != file.size || hex_digest(digest.finalize()) != file.sha256 {
                return Err(BrowserRuntimeError::new(format!(
                    "embedded runtime payload verification failed for {name}"
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                output_file
                    .set_permissions(fs::Permissions::from_mode(expected_mode))
                    .map_err(|error| {
                        BrowserRuntimeError::new(format!(
                            "could not set runtime permissions on {}: {error}",
                            output.display()
                        ))
                    })?;
            }
            output_file.sync_all().map_err(|error| {
                BrowserRuntimeError::new(format!(
                    "could not sync extracted runtime file {}: {error}",
                    output.display()
                ))
            })?;
        }
        RuntimeFileKind::Symlink => {
            let mut target = String::new();
            entry.read_to_string(&mut target).map_err(|error| {
                BrowserRuntimeError::new(format!("could not read runtime symlink {name}: {error}"))
            })?;
            validate_symlink_target(&file.path, &target)?;
            if file.target.as_deref() != Some(target.as_str())
                || target.len() as u64 != file.size
                || hash_bytes(target.as_bytes()) != file.sha256
            {
                return Err(BrowserRuntimeError::new(format!(
                    "embedded runtime symlink verification failed for {name}"
                )));
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &output).map_err(|error| {
                BrowserRuntimeError::new(format!(
                    "could not create runtime symlink {}: {error}",
                    output.display()
                ))
            })?;
            #[cfg(not(unix))]
            return Err(BrowserRuntimeError::new(
                "the embedded Camoufox Linux runtime cannot be extracted on this platform",
            ));
        }
    }
    Ok(())
}

fn replace_runtime_directory(root: &Path, temporary: &Path) -> Result<(), BrowserRuntimeError> {
    static NEXT_BACKUP: AtomicUsize = AtomicUsize::new(1);
    let backup = root.with_file_name(format!(
        ".runtime.invalid.{}.{}",
        std::process::id(),
        NEXT_BACKUP.fetch_add(1, Ordering::Relaxed)
    ));
    let had_existing = fs::symlink_metadata(root).is_ok();
    if had_existing {
        fs::rename(root, &backup).map_err(|error| {
            BrowserRuntimeError::new(format!(
                "could not quarantine invalid runtime {}: {error}",
                root.display()
            ))
        })?;
    }
    if let Err(error) = fs::rename(temporary, root) {
        if had_existing {
            let _ = fs::rename(&backup, root);
        }
        return Err(BrowserRuntimeError::new(format!(
            "could not install Camoufox runtime at {}: {error}",
            root.display()
        )));
    }
    if had_existing {
        let _ = fs::remove_dir_all(backup);
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct BrowserProfileLease {
    path: PathBuf,
    _shared_lock: Option<fs::File>,
    _isolated_dir: Option<TempDir>,
}

impl BrowserProfileLease {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    fn is_isolated(&self) -> bool {
        self._isolated_dir.is_some()
    }
}

pub(crate) fn acquire_browser_profile() -> Result<BrowserProfileLease, BrowserRuntimeError> {
    acquire_browser_profile_at(&code_home().join("browser"))
}

fn acquire_browser_profile_at(
    browser_root: &Path,
) -> Result<BrowserProfileLease, BrowserRuntimeError> {
    fs::create_dir_all(browser_root).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not create browser data directory {}: {error}",
            browser_root.display()
        ))
    })?;

    let shared_profile = browser_root.join("profile");
    let lock_path = browser_root.join(PROFILE_LEASE_FILE);
    let lock_file = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "rustwright_profile_lease_warning path={} error={error}",
                lock_path.display()
            );
            return isolated_browser_profile(browser_root);
        }
    };

    match lock_file.try_lock_exclusive() {
        Ok(()) if !firefox_profile_has_lock(&shared_profile) => {
            fs::create_dir_all(&shared_profile).map_err(|error| {
                BrowserRuntimeError::new(format!(
                    "could not create browser profile {}: {error}",
                    shared_profile.display()
                ))
            })?;
            Ok(BrowserProfileLease {
                path: shared_profile,
                _shared_lock: Some(lock_file),
                _isolated_dir: None,
            })
        }
        Ok(()) => {
            let _ = FileExt::unlock(&lock_file);
            isolated_browser_profile(browser_root)
        }
        Err(error) => {
            eprintln!(
                "rustwright_profile_lease_warning path={} error={error}",
                lock_path.display()
            );
            isolated_browser_profile(browser_root)
        }
    }
}

fn firefox_profile_has_lock(profile: &Path) -> bool {
    FIREFOX_PROFILE_LOCKS
        .iter()
        .any(|name| profile.join(name).symlink_metadata().is_ok())
}

fn isolated_browser_profile(
    browser_root: &Path,
) -> Result<BrowserProfileLease, BrowserRuntimeError> {
    let isolated_dir = tempfile::Builder::new()
        .prefix("profile-isolated-")
        .tempdir_in(browser_root)
        .map_err(|error| {
            BrowserRuntimeError::new(format!(
                "could not create isolated browser profile in {}: {error}",
                browser_root.display()
            ))
        })?;
    let path = isolated_dir.path().to_path_buf();
    eprintln!(
        "rustwright_profile_isolated shared_profile_busy=true path={}",
        path.display()
    );
    Ok(BrowserProfileLease {
        path,
        _shared_lock: None,
        _isolated_dir: Some(isolated_dir),
    })
}

fn code_home() -> PathBuf {
    if let Some(path) = env::var_os("CODE_HOME") {
        return PathBuf::from(path);
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".code")
}

fn base_launch_options(
    runtime: &BrowserRuntime,
    persistent_profile: Option<PathBuf>,
) -> LaunchOptions {
    LaunchOptions {
        os: vec![SupportedOs::Linux],
        headless: HeadlessMode::On,
        main_world_eval: true,
        executable_path: Some(runtime.executable().to_path_buf()),
        ff_version: Some(runtime.firefox_major.clone()),
        exclude_addons: vec![DefaultAddon::Ubo],
        env: runtime.child_environment(),
        persistent_profile,
        i_know_what_im_doing: true,
        ..LaunchOptions::default()
    }
}

pub(crate) async fn prepare_browser_launch(
    runtime: &BrowserRuntime,
    persistent_profile: Option<PathBuf>,
) -> Result<PreparedBrowserLaunch, BrowserRuntimeError> {
    let options = base_launch_options(runtime, persistent_profile);
    let prepared = prepare(&options).await.map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not prepare Camoufox fingerprint configuration: {error}"
        ))
    })?;
    Ok(PreparedBrowserLaunch {
        options,
        prepared: resizable_prepared_launch(prepared)?,
    })
}

fn resizable_prepared_launch(
    mut prepared: PreparedLaunch,
) -> Result<PreparedLaunch, BrowserRuntimeError> {
    prepared.env = resizable_camoufox_environment(&prepared.env)?;
    for key in WINDOW_DIM_KEYS {
        prepared.config.remove(*key);
    }
    prepared.spoofs_window_dimensions = false;
    Ok(prepared)
}

fn resizable_camoufox_environment(
    generated: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, BrowserRuntimeError> {
    let mut chunks = generated
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix(CAMOU_CONFIG_PREFIX)
                .and_then(|suffix| suffix.parse::<usize>().ok())
                .map(|index| (index, value))
        })
        .collect::<Vec<_>>();
    chunks.sort_unstable_by_key(|(index, _)| *index);
    if chunks.is_empty()
        || chunks
            .iter()
            .enumerate()
            .any(|(offset, (index, _))| *index != offset + 1)
    {
        return Err(BrowserRuntimeError::new(
            "Camoufox generated an incomplete CAMOU_CONFIG sequence",
        ));
    }

    let encoded = chunks
        .iter()
        .map(|(_, value)| value.as_str())
        .collect::<String>();
    let mut config = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&encoded)
        .map_err(|error| {
        BrowserRuntimeError::new(format!(
            "Camoufox generated invalid fingerprint configuration: {error}"
        ))
    })?;
    for key in WINDOW_DIM_KEYS {
        config.remove(*key);
    }
    let encoded = serde_json::to_string(&config).map_err(|error| {
        BrowserRuntimeError::new(format!(
            "could not serialize resizable Camoufox fingerprint configuration: {error}"
        ))
    })?;

    let chunk_size = if cfg!(windows) { 2047 } else { 32767 };
    let characters = encoded.chars().collect::<Vec<_>>();
    let mut sanitized = generated.clone();
    sanitized.retain(|key, _| !key.starts_with(CAMOU_CONFIG_PREFIX));
    for (offset, chunk) in characters.chunks(chunk_size).enumerate() {
        sanitized.insert(
            format!("{CAMOU_CONFIG_PREFIX}{}", offset + 1),
            chunk.iter().collect(),
        );
    }
    for index in characters.chunks(chunk_size).count() + 1..=chunks.len() + 1 {
        sanitized.insert(format!("{CAMOU_CONFIG_PREFIX}{index}"), String::new());
    }
    Ok(sanitized)
}

pub(crate) fn validate_screenshot(bytes: &[u8]) -> Result<(), BrowserRuntimeError> {
    let image = image::load_from_memory(bytes)
        .map_err(|error| BrowserRuntimeError::new(format!("invalid browser screenshot: {error}")))?
        .to_rgb8();
    let pixel_count = u64::from(image.width()) * u64::from(image.height());
    if pixel_count == 0 {
        return Err(BrowserRuntimeError::new("browser screenshot is empty"));
    }

    let mut colors = std::collections::HashMap::<[u8; 3], u64>::new();
    for pixel in image.pixels() {
        *colors.entry(pixel.0).or_default() += 1;
    }
    if let Some((color, dominant_count)) = colors.into_iter().max_by_key(|(_, count)| *count) {
        let dominance = dominant_count as f64 / pixel_count as f64;
        let luminance = (u16::from(color[0]) + u16::from(color[1]) + u16::from(color[2])) / 3;
        if dominance >= 0.98 && luminance <= 32 {
            return Err(BrowserRuntimeError::uniform_dark_frame(dominance, color));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::env;
    use std::fs;
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::thread;

    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use camoufox::builder::{HeadlessMode, PreparedLaunch};
    use camoufox_core::os::SupportedOs;
    use camoufox_pkgman::DefaultAddon;

    use super::{
        BrowserRuntime, RuntimeFile, RuntimeFileKind, acquire_browser_profile_at,
        base_launch_options, extract_runtime_file, resizable_camoufox_environment,
        resizable_prepared_launch, validate_screenshot,
    };

    #[test]
    fn concurrent_browser_clients_receive_distinct_profiles() {
        let root = tempdir().unwrap();

        let shared = acquire_browser_profile_at(root.path()).unwrap();
        assert_eq!(shared.path(), root.path().join("profile"));
        assert!(!shared.is_isolated());

        let isolated = acquire_browser_profile_at(root.path()).unwrap();
        let isolated_path = isolated.path().to_path_buf();
        assert_ne!(isolated.path(), shared.path());
        assert!(isolated.is_isolated());
        assert!(isolated_path.is_dir());

        drop(isolated);
        assert!(!isolated_path.exists());

        drop(shared);
        let next = acquire_browser_profile_at(root.path()).unwrap();
        assert_eq!(next.path(), root.path().join("profile"));
        assert!(!next.is_isolated());
    }

    #[test]
    fn legacy_firefox_profile_lock_forces_an_isolated_profile() {
        let root = tempdir().unwrap();
        let shared_profile = root.path().join("profile");
        fs::create_dir_all(&shared_profile).unwrap();
        fs::write(shared_profile.join("lock"), b"legacy owner").unwrap();

        let lease = acquire_browser_profile_at(root.path()).unwrap();
        assert_ne!(lease.path(), shared_profile);
        assert!(lease.is_isolated());
    }

    #[test]
    fn runtime_tree_digest_matches_the_packager_contract() {
        let file = RuntimeFile {
            path: "camoufox/camoufox-bin".to_owned(),
            kind: RuntimeFileKind::File,
            target: None,
            size: 7,
            sha256: "0123456789abcdef".to_owned(),
            mode: "0755".to_owned(),
        };
        let mut digest = Sha256::new();
        super::update_tree_digest(&mut digest, &file);
        assert_eq!(
            super::hex_digest(digest.finalize()),
            "5a17652ca72b070b873f7d74d72b08e1320f7d55d91d932ccc8c7f6f90534d37"
        );
    }

    #[test]
    fn runtime_hashing_fits_the_browser_actor_stack() {
        const CHILD_ENV: &str = "CODE_TEST_CAMOUFOX_SMALL_STACK_HASH";
        const TEST_NAME: &str =
            "camoufox_runtime::tests::runtime_hashing_fits_the_browser_actor_stack";

        if let Some(path) = env::var_os(CHILD_ENV) {
            let path = PathBuf::from(path);
            let result = thread::Builder::new()
                .name("camoufox-small-stack-hash".to_owned())
                .stack_size(512 * 1024)
                .spawn(move || super::hash_file(&path))
                .expect("spawn small-stack hashing thread")
                .join()
                .expect("small-stack hashing thread must not overflow");
            result.expect("hash runtime file on the actor-sized stack");
            return;
        }

        let temp = tempdir().unwrap();
        let payload = temp.path().join("large-runtime-file");
        fs::write(&payload, vec![0x5a; 2 * 1024 * 1024]).unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .arg("--exact")
            .arg(TEST_NAME)
            .env(CHILD_ENV, &payload)
            .status()
            .expect("run isolated small-stack hashing regression");
        assert!(
            status.success(),
            "runtime hashing overflowed a small actor stack"
        );
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("runtime path parent")).unwrap();
        fs::write(&path, b"fixture").unwrap();
        #[cfg(unix)]
        if relative == "camoufox/camoufox-bin" {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn complete_runtime(root: &Path) {
        for relative in fixture_paths() {
            touch(root, relative);
        }
        write_manifest(root, "152.0.4");
    }

    fn fixture_paths() -> &'static [&'static str] {
        &[
            "camoufox/camoufox-bin",
            "camoufox/camoufox.cfg",
            "camoufox/camoucfg.jvv",
            "camoufox/properties.json",
            "camoufox/GeoLite2-City.mmdb",
            "lib/libEGL.so.1",
            "lib/libGL.so.1",
            "lib/libGLX_mesa.so.0",
            "lib/libLLVM.so",
            "lib/dri/swrast_dri.so",
            "licenses/camoufox-MPL-2.0.txt",
            "licenses/mesa-MIT.txt",
            "licenses/llvm-Apache-2.0-WITH-LLVM-exception.txt",
        ]
    }

    fn write_manifest(root: &Path, firefox_version: &str) {
        let mut files = Vec::new();
        let mut tree = Sha256::new();
        for relative in fixture_paths() {
            let path = root.join(relative);
            let metadata = path.metadata().unwrap();
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o7777
            };
            #[cfg(not(unix))]
            let mode = if *relative == "camoufox/camoufox-bin" {
                0o755
            } else {
                0o644
            };
            let hash = super::hash_file(&path).unwrap();
            let mode = format!("{mode:04o}");
            let size = metadata.len();
            let record = serde_json::json!({
                "path": relative,
                "type": "file",
                "size": size,
                "sha256": hash,
                "mode": mode,
            });
            for (index, field) in [
                "file",
                *relative,
                mode.as_str(),
                size.to_string().as_str(),
                hash.as_str(),
                "",
            ]
            .into_iter()
            .enumerate()
            {
                if index > 0 {
                    tree.update([0]);
                }
                tree.update(field.as_bytes());
            }
            tree.update(b"\n");
            files.push(record);
        }
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schemaVersion": 2,
                "target": "x86_64-unknown-linux-gnu",
                "archivePrefix": "browser-runtime/",
                "camoufox": {
                    "version": "152.0.4",
                    "release": "beta.28",
                    "sourceRevision": "0583c3ec94f5a9df5cb2d09553fbfe80589b6e2d",
                    "rustRevision": "933ae2e8ba7a00e254b4d4075eb0e85a527289fc"
                },
                "automation": {
                    "protocol": "juggler-pipe",
                    "driver": "camoufox-juggler",
                    "driverRevision": "933ae2e8ba7a00e254b4d4075eb0e85a527289fc",
                    "entrypoint": "rustwright-mcp",
                    "transport": "mcp-stdio-json-rpc"
                },
                "firefox": { "version": firefox_version },
                "mesa": { "version": "26.2.4", "sourceRevision": "host-prototype" },
                "llvm": { "version": "22.1", "soname": "libLLVM.so.22.1" },
                "treeSha256": super::hex_digest(tree.finalize()),
                "files": files
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn complete_runtime_selects_bundled_camoufox_and_isolates_software_mesa() {
        let temp = tempdir().unwrap();
        complete_runtime(temp.path());

        let runtime = BrowserRuntime::open(temp.path()).expect("complete runtime");
        assert_eq!(
            runtime.executable(),
            temp.path().join("camoufox/camoufox-bin")
        );

        let environment: BTreeMap<_, _> = runtime.child_environment().into_iter().collect();
        assert_eq!(
            environment.get("MOZ_HEADLESS").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            environment.get("LIBGL_ALWAYS_SOFTWARE").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            environment.get("GALLIUM_DRIVER").map(String::as_str),
            Some("llvmpipe")
        );
        assert_eq!(
            environment
                .get("MESA_LOADER_DRIVER_OVERRIDE")
                .map(String::as_str),
            Some("llvmpipe")
        );
        assert_eq!(
            environment.get("EGL_PLATFORM").map(String::as_str),
            Some("surfaceless")
        );
        assert_eq!(
            environment.get("LIBGL_DRIVERS_PATH"),
            Some(&temp.path().join("lib/dri").to_string_lossy().into_owned())
        );
        assert_eq!(
            environment.get("FONTCONFIG_PATH"),
            Some(
                &temp
                    .path()
                    .join("camoufox/fontconfig/linux")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert!(environment.get("LD_LIBRARY_PATH").is_some_and(|value| {
            value.starts_with(&temp.path().join("lib").to_string_lossy().into_owned())
        }));
        assert!(!environment.contains_key("DISPLAY"));
    }

    #[test]
    fn launch_contract_uses_true_headless_juggler_without_virtual_display() {
        let temp = tempdir().unwrap();
        complete_runtime(temp.path());
        write_manifest(temp.path(), "147.2.1");
        let runtime = BrowserRuntime::open(temp.path()).unwrap();

        let options = base_launch_options(&runtime, None);
        assert_eq!(options.headless, HeadlessMode::On);
        assert_ne!(options.headless, HeadlessMode::Virtual);
        assert_eq!(
            options.executable_path.as_deref(),
            Some(runtime.executable())
        );
        assert_eq!(options.os, vec![SupportedOs::Linux]);
        assert_eq!(options.ff_version.as_deref(), Some("147"));
        assert_eq!(options.exclude_addons, vec![DefaultAddon::Ubo]);
        assert!(options.main_world_eval);
        assert!(!options.env.contains_key("DISPLAY"));
        assert!(
            !options
                .args
                .iter()
                .any(|argument| argument.contains("remote-debugging"))
        );
    }

    #[test]
    fn incomplete_runtime_reports_every_missing_payload_class() {
        let temp = tempdir().unwrap();
        touch(temp.path(), "camoufox/camoufox-bin");

        let error = BrowserRuntime::open(temp.path()).unwrap_err().to_string();
        for expected in [
            "manifest.json",
            "libEGL.so.1",
            "libGL.so.1",
            "libGLX_mesa.so.0",
            "libLLVM.so",
            "swrast_dri.so",
            "camoufox-MPL-2.0.txt",
            "mesa-MIT.txt",
            "llvm-Apache-2.0-WITH-LLVM-exception.txt",
        ] {
            assert!(
                error.contains(expected),
                "missing diagnostic for {expected}: {error}"
            );
        }
    }

    #[test]
    fn overwhelmingly_uniform_dark_screenshot_is_rejected() {
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(64, 64, Rgba([18, 18, 18, 255])));
        let mut bytes = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        let error = validate_screenshot(&bytes).unwrap_err();
        assert!(error.is_uniform_dark_frame());
        assert!(error.to_string().contains("uniform dark frame"), "{error}");
    }

    #[test]
    fn corrupt_screenshot_is_not_classified_as_a_dark_compositor_frame() {
        let error = validate_screenshot(b"truncated PNG").unwrap_err();

        assert!(!error.is_uniform_dark_frame());
        assert!(error.to_string().contains("invalid browser screenshot"));
    }

    #[test]
    fn ordinary_screenshot_is_accepted() {
        let image = DynamicImage::ImageRgba8(ImageBuffer::from_fn(64, 64, |x, y| {
            if (x + y) % 3 == 0 {
                Rgba([235, 245, 255, 255])
            } else {
                Rgba([30, 80, 140, 255])
            }
        }));
        let mut bytes = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();

        validate_screenshot(&bytes).expect("non-uniform screenshot");
    }

    #[test]
    fn generated_fingerprint_keeps_screen_identity_but_releases_window_dimensions() {
        let generated = BTreeMap::from([
            (
                "CAMOU_CONFIG_1".to_owned(),
                serde_json::json!({
                    "navigator.userAgent": "fixture",
                    "screen.width": 1920,
                    "screen.height": 1080,
                    "window.outerWidth": 1920,
                    "window.outerHeight": 1040,
                    "window.innerWidth": 1920,
                    "window.innerHeight": 985,
                    "document.body.clientWidth": 1920,
                    "document.body.clientHeight": 985,
                    "canvas:seed": 7
                })
                .to_string(),
            ),
            (
                "FONTCONFIG_PATH".to_owned(),
                "/runtime/fontconfig".to_owned(),
            ),
        ]);

        let sanitized =
            resizable_camoufox_environment(&generated).expect("sanitize generated config");
        let config: serde_json::Value = serde_json::from_str(
            sanitized
                .get("CAMOU_CONFIG_1")
                .expect("sanitized Camoufox config"),
        )
        .unwrap();

        assert_eq!(config["navigator.userAgent"], "fixture");
        assert_eq!(config["screen.width"], 1920);
        assert_eq!(config["screen.height"], 1080);
        assert_eq!(config["canvas:seed"], 7);
        for key in [
            "window.outerWidth",
            "window.outerHeight",
            "window.innerWidth",
            "window.innerHeight",
            "document.body.clientWidth",
            "document.body.clientHeight",
        ] {
            assert!(
                config.get(key).is_none(),
                "window dimension remained: {key}"
            );
        }
        assert_eq!(
            sanitized.get("FONTCONFIG_PATH").map(String::as_str),
            Some("/runtime/fontconfig")
        );
        assert_eq!(
            sanitized.get("CAMOU_CONFIG_2").map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn prepared_launch_keeps_the_complete_camoufox_and_juggler_contract() {
        let config = serde_json::json!({
            "navigator.userAgent": "fixture",
            "navigator.hardwareConcurrency": 8,
            "screen.width": 1920,
            "screen.height": 1080,
            "window.outerWidth": 1920,
            "window.outerHeight": 1040,
            "window.innerWidth": 1920,
            "window.innerHeight": 985,
            "document.body.clientWidth": 1920,
            "document.body.clientHeight": 985,
            "webGl:vendor": "fixture-vendor",
            "webGl:renderer": "fixture-renderer",
            "canvas:seed": 7,
            "audio:seed": 9,
            "fonts:spacing_seed": 11,
            "allowMainWorld": true
        })
        .as_object()
        .unwrap()
        .clone();
        let prepared = PreparedLaunch {
            executable_path: PathBuf::from("/runtime/camoufox-bin"),
            env: BTreeMap::from([
                (
                    "CAMOU_CONFIG_1".to_owned(),
                    serde_json::to_string(&config).unwrap(),
                ),
                (
                    "FONTCONFIG_PATH".to_owned(),
                    "/runtime/fontconfig".to_owned(),
                ),
            ]),
            firefox_user_prefs: BTreeMap::from([
                (
                    "webgl.force-enabled".to_owned(),
                    serde_json::Value::Bool(true),
                ),
                (
                    "webgl.enable-webgl2".to_owned(),
                    serde_json::Value::Bool(true),
                ),
            ]),
            args: vec!["--fixture-flag".to_owned()],
            proxy: Some("http://proxy.example".to_owned()),
            proxy_bypass: Some("localhost".to_owned()),
            spoofs_window_dimensions: true,
            config,
        };

        let prepared = resizable_prepared_launch(prepared).expect("resizable prepared launch");

        for key in [
            "navigator.userAgent",
            "navigator.hardwareConcurrency",
            "screen.width",
            "screen.height",
            "webGl:vendor",
            "webGl:renderer",
            "canvas:seed",
            "audio:seed",
            "fonts:spacing_seed",
            "allowMainWorld",
        ] {
            assert!(
                prepared.config.contains_key(key),
                "Camouflag was dropped: {key}"
            );
        }
        for key in super::WINDOW_DIM_KEYS {
            assert!(
                !prepared.config.contains_key(*key),
                "resizing-conflicting Camouflag remained: {key}"
            );
        }
        assert!(!prepared.spoofs_window_dimensions);
        assert_eq!(prepared.args, ["--fixture-flag"]);
        assert_eq!(prepared.proxy.as_deref(), Some("http://proxy.example"));
        assert_eq!(prepared.proxy_bypass.as_deref(), Some("localhost"));
        assert_eq!(
            prepared.firefox_user_prefs.get("webgl.force-enabled"),
            Some(&serde_json::Value::Bool(true))
        );
    }

    #[test]
    fn embedded_archive_extracts_verifies_and_replaces_invalid_cache() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        complete_runtime(&source);
        let archive_path = temp.path().join("code.com");
        let mut executable = fs::File::create(&archive_path).unwrap();
        executable.write_all(b"MZqFpD fixture\n").unwrap();
        let mut archive = ZipWriter::new(executable);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        archive
            .start_file(
                "browser-runtime/manifest.json",
                options.unix_permissions(0o644),
            )
            .unwrap();
        archive
            .write_all(&fs::read(source.join("manifest.json")).unwrap())
            .unwrap();
        for relative in fixture_paths() {
            let mode = if *relative == "camoufox/camoufox-bin" {
                0o755
            } else {
                0o644
            };
            archive
                .start_file(
                    format!("browser-runtime/{relative}"),
                    options.unix_permissions(mode),
                )
                .unwrap();
            archive
                .write_all(&fs::read(source.join(relative)).unwrap())
                .unwrap();
        }
        archive.finish().unwrap();

        let installed = temp.path().join("cache/runtime");
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("corrupt"), b"old").unwrap();
        super::extract_embedded_runtime(&archive_path, &installed).unwrap();

        let runtime = BrowserRuntime::open(&installed).unwrap();
        assert_eq!(
            runtime.executable(),
            installed.join("camoufox/camoufox-bin")
        );
        assert!(!installed.join("corrupt").exists());
    }

    #[test]
    fn extraction_rejects_archive_entry_type_mismatch() {
        let temp = tempdir().unwrap();
        let archive_path = temp.path().join("runtime.zip");
        let output = fs::File::create(&archive_path).unwrap();
        let mut writer = ZipWriter::new(output);
        writer
            .start_file(
                "browser-runtime/lib/libLLVM.so",
                SimpleFileOptions::default().unix_permissions(0o777),
            )
            .unwrap();
        writer.write_all(b"libLLVM.so.22.1").unwrap();
        writer.finish().unwrap();

        let input = fs::File::open(&archive_path).unwrap();
        let mut archive = zip::ZipArchive::new(input).unwrap();
        let file = RuntimeFile {
            path: "lib/libLLVM.so".to_owned(),
            kind: RuntimeFileKind::Symlink,
            target: Some("libLLVM.so.22.1".to_owned()),
            size: 17,
            sha256: super::hash_bytes(b"libLLVM.so.22.1"),
            mode: "0777".to_owned(),
        };

        let error = extract_runtime_file(&mut archive, temp.path(), &file)
            .unwrap_err()
            .to_string();
        assert!(error.contains("entry type mismatch"), "{error}");
    }
}
