//! Immutable, explicit material snapshots. No source root is exposed to workers.
use crate::runtime::{OutputLock, atomic_json, atomic_text, normalize_path, private_dir, sha256};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::{CStr, CString, OsString},
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStringExt, fs::OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};
use uuid::Uuid;

pub const MAX_PACKET_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_API_PACKET_BYTES: usize = 512 * 1024;
pub const MAX_FILE_BYTES: usize = 512 * 1024;
pub const MAX_FILES: usize = 2_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PacketFile {
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
    pub lines: usize,
    pub blob: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Exclusion {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PacketManifest {
    pub schema_version: u32,
    pub packet_hash: String,
    pub files: Vec<PacketFile>,
    pub exclusions: Vec<Exclusion>,
    pub total_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct Packet {
    pub dir: PathBuf,
    pub manifest: PacketManifest,
}

impl PacketManifest {
    fn identity(&self) -> Result<String> {
        Ok(sha256(&serde_json::to_vec(&(
            self.schema_version,
            &self.files,
            &self.exclusions,
            self.total_bytes,
        ))?))
    }
}

impl Packet {
    pub fn load(dir: &Path) -> Result<Self> {
        if fs::symlink_metadata(dir)?.file_type().is_symlink() {
            bail!("packet directory must not be a symlink");
        }
        let dir = fs::canonicalize(dir)?;
        let bytes = read_beneath(&dir, "manifest.json", 4 * 1024 * 1024)?;
        let manifest: PacketManifest =
            serde_json::from_slice(&bytes).context("invalid packet manifest")?;
        let packet = Self { dir, manifest };
        packet.verify()?;
        Ok(packet)
    }

    pub fn verify(&self) -> Result<()> {
        let disk: PacketManifest =
            serde_json::from_slice(&read_beneath(&self.dir, "manifest.json", 4 * 1024 * 1024)?)?;
        if disk.packet_hash != self.manifest.packet_hash
            || disk.identity()? != self.manifest.packet_hash
        {
            bail!("packet manifest changed or its hash is invalid");
        }
        self.verify_manifest()?;
        for file in &self.manifest.files {
            self.read(&file.path)?;
        }
        Ok(())
    }

    fn verify_manifest(&self) -> Result<()> {
        let m = &self.manifest;
        if m.schema_version != 1 || m.packet_hash != m.identity()? {
            bail!("unsupported or damaged packet manifest");
        }
        if m.files.len() > MAX_FILES || m.total_bytes > MAX_PACKET_BYTES {
            bail!("packet exceeds material limits");
        }
        let mut previous: Option<&str> = None;
        let mut total = 0usize;
        for file in &m.files {
            validate_relative(&file.path)?;
            if previous.is_some_and(|p| p >= file.path.as_str()) {
                bail!("packet paths must be unique and sorted");
            }
            previous = Some(&file.path);
            if file.bytes > MAX_FILE_BYTES
                || !valid_hash(&file.sha256)
                || file.blob != format!("blobs/{}.txt", file.sha256)
            {
                bail!("invalid packet entry for {}", file.path);
            }
            total = total
                .checked_add(file.bytes)
                .context("packet size overflow")?;
        }
        if total != m.total_bytes {
            bail!("packet size does not match manifest");
        }
        Ok(())
    }

    pub fn read(&self, path: &str) -> Result<String> {
        validate_relative(path)?;
        let file = self
            .manifest
            .files
            .iter()
            .find(|f| f.path == path)
            .context("path is outside the authorized packet")?;
        let bytes = read_beneath(&self.dir, &file.blob, MAX_FILE_BYTES)?;
        if bytes.len() != file.bytes || sha256(&bytes) != file.sha256 {
            bail!("packet content hash mismatch: {path}");
        }
        let text = String::from_utf8(bytes).context("packet contains invalid UTF-8")?;
        if text.lines().count() != file.lines {
            bail!("packet line count mismatch: {path}");
        }
        Ok(text)
    }

    pub fn total_bytes(&self) -> usize {
        self.manifest.total_bytes
    }
    pub fn hash(&self) -> &str {
        &self.manifest.packet_hash
    }
}

pub fn create(root: &Path, inputs: &[String], scopes: &[String], outdir: &Path) -> Result<Packet> {
    if inputs.is_empty() && scopes.is_empty() {
        bail!("pack requires explicit --input or --scope");
    }
    if fs::symlink_metadata(root)?.file_type().is_symlink() {
        bail!("material root must not be a symlink");
    }
    let root = fs::canonicalize(root).context("invalid material root")?;
    if !root.is_dir() {
        bail!("material root must be a directory");
    }
    let outdir = normalize_path(outdir)?;
    // All validation and reads precede output writes; lock acquisition precedes any state mutation.
    let _lock = OutputLock::acquire(&[(outdir.clone(), true)])?;
    reject_existing_symlink(outdir.as_path())?;
    if outdir.exists() {
        bail!("packet output already exists; choose a new immutable packet directory");
    }
    let mut collector = Collector {
        root: &root,
        output: &outdir,
        files: BTreeMap::new(),
        exclusions: BTreeMap::new(),
        total: 0,
    };
    for input in inputs {
        let relative = selection_path(&root, input)?;
        collector.collect(&relative, false)?;
    }
    for scope in scopes {
        let relative = selection_path(&root, scope)?;
        collector.collect(&relative, true)?;
    }
    if collector.files.is_empty() {
        bail!("authorized material contains no eligible text files");
    }
    let files: Vec<PacketFile> = collector
        .files
        .iter()
        .map(|(path, bytes)| {
            let hash = sha256(bytes);
            PacketFile {
                path: path.clone(),
                sha256: hash.clone(),
                bytes: bytes.len(),
                lines: std::str::from_utf8(bytes).unwrap().lines().count(),
                blob: format!("blobs/{hash}.txt"),
            }
        })
        .collect();
    let mut manifest = PacketManifest {
        schema_version: 1,
        packet_hash: String::new(),
        files,
        exclusions: collector
            .exclusions
            .iter()
            .map(|(path, reason)| Exclusion {
                path: path.clone(),
                reason: reason.clone(),
            })
            .collect(),
        total_bytes: collector.total,
    };
    manifest.packet_hash = manifest.identity()?;
    let parent = outdir.parent().context("packet output needs a parent")?;
    // Do not chmod an existing user/source parent merely to create a private child.
    if !parent.exists() {
        private_dir(parent)?;
    }
    let stage = parent.join(format!(".multi-ai-packet-{}", Uuid::new_v4()));
    let result = (|| -> Result<()> {
        private_dir(&stage)?;
        private_dir(&stage.join("blobs"))?;
        for file in &manifest.files {
            let text = std::str::from_utf8(&collector.files[&file.path])?;
            atomic_text(&stage.join(&file.blob), text)?;
        }
        atomic_json(
            &stage.join("manifest.json"),
            &serde_json::to_value(&manifest)?,
        )?;
        fs::rename(&stage, &outdir)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result?;
    Packet::load(&outdir)
}

struct Collector<'a> {
    root: &'a Path,
    output: &'a Path,
    files: BTreeMap<String, Vec<u8>>,
    exclusions: BTreeMap<String, String>,
    total: usize,
}

impl Collector<'_> {
    fn collect(&mut self, relative: &Path, recurse: bool) -> Result<()> {
        let path = self.root.join(relative);
        let display = relative
            .to_str()
            .context("material paths must be valid UTF-8")?
            .replace('\\', "/");
        if path.starts_with(self.output) || path.starts_with(crate::runtime::state_dir()) {
            self.exclusions.insert(display, "managed-output".into());
            return Ok(());
        }
        if let Some(reason) = exclusion_reason(relative) {
            self.exclusions.insert(display, reason.into());
            return Ok(());
        }
        // Opening each component with O_NOFOLLOW prevents a concurrently replaced parent escaping root.
        let file = open_beneath(self.root, &display)?;
        let metadata = file.metadata()?;
        if metadata.is_dir() {
            if !recurse {
                bail!("--input must name a file: {display}; use --scope for directories");
            }
            let mut children = directory_children(&file)?;
            children.sort();
            for child in children {
                self.collect(&relative.join(child), true)?;
            }
        } else if metadata.is_file() {
            if self.files.contains_key(&display) {
                return Ok(());
            }
            if metadata.len() > MAX_FILE_BYTES as u64 {
                bail!("material file exceeds 512 KiB: {display}");
            }
            let mut bytes = Vec::new();
            file.take((MAX_FILE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_FILE_BYTES {
                bail!("material file exceeds 512 KiB: {display}");
            }
            if bytes.contains(&0) || std::str::from_utf8(&bytes).is_err() {
                self.exclusions
                    .insert(display, "binary-or-invalid-utf8".into());
                return Ok(());
            }
            if looks_like_credentials(&bytes) {
                self.exclusions.insert(display, "credential-content".into());
                return Ok(());
            }
            if self.files.len() >= MAX_FILES {
                bail!("material packet exceeds 2,000 files");
            }
            self.total = self
                .total
                .checked_add(bytes.len())
                .context("material size overflow")?;
            if self.total > MAX_PACKET_BYTES {
                bail!("material packet exceeds 8 MiB");
            }
            self.files.insert(display, bytes);
        } else {
            bail!("material must be a regular file or directory: {display}");
        }
        Ok(())
    }
}

fn looks_like_credentials(bytes: &[u8]) -> bool {
    static KEY: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{16,}|AKIA[A-Z0-9]{16}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{40,}|xox[baprs]-[A-Za-z0-9-]{20,})\b").unwrap()
    });
    let text = std::str::from_utf8(bytes).unwrap_or_default();
    text.contains("-----BEGIN PRIVATE KEY-----")
        || text.contains("-----BEGIN RSA PRIVATE KEY-----")
        || text.contains("-----BEGIN OPENSSH PRIVATE KEY-----")
        || KEY.is_match(text)
        || std::env::vars().any(|(name, value)| {
            value.len() >= 8
                && ["KEY", "TOKEN", "PASSWORD", "SECRET"]
                    .iter()
                    .any(|needle| name.to_uppercase().contains(needle))
                && text.contains(&value)
        })
}

fn exclusion_reason(path: &Path) -> Option<&'static str> {
    for component in path.components() {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            ".git"
                | "node_modules"
                | "target"
                | "dist"
                | "build"
                | "vendor"
                | ".next"
                | ".cache"
                | "__pycache__"
        ) {
            return Some("build-vendor-or-vcs");
        }
        if matches!(
            name.as_str(),
            ".grok"
                | ".kimi"
                | ".kimi-code"
                | ".claude"
                | ".cursor"
                | ".codex"
                | ".agents"
                | ".ssh"
                | ".aws"
                | ".config"
        ) {
            return Some("credential-or-extension-config");
        }
        if name.starts_with(".env")
            || matches!(
                name.as_str(),
                "credentials"
                    | "credentials.json"
                    | "auth.json"
                    | "oauth.json"
                    | "token.json"
                    | "tokens.json"
                    | "login.json"
                    | "id_rsa"
                    | "id_ed25519"
            )
            || [".pem", ".key", ".p12", ".pfx", ".keystore"]
                .iter()
                .any(|suffix| name.ends_with(suffix))
        {
            return Some("credential-file");
        }
    }
    None
}

fn selection_path(root: &Path, selection: &str) -> Result<PathBuf> {
    let path = Path::new(selection);
    let relative = if path.is_absolute() {
        path.strip_prefix(root)
            .context("material path escapes root")?
    } else {
        path
    };
    if relative == Path::new(".") || relative.as_os_str().is_empty() {
        return Ok(PathBuf::new());
    }
    let text = relative.to_str().context("material path is not UTF-8")?;
    validate_relative(text)?;
    Ok(relative.to_path_buf())
}

pub fn validate_relative(path: &str) -> Result<()> {
    if path.is_empty()
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        bail!("only canonical packet-relative paths are allowed");
    }
    Ok(())
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn reject_existing_symlink(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!("managed packet output must not be a symlink");
    }
    Ok(())
}

fn open_beneath(root: &Path, relative: &str) -> Result<File> {
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(root)?;
    if relative.is_empty() {
        return Ok(current);
    }
    validate_relative(relative)?;
    let parts = Path::new(relative).components().collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        let name = CString::new(part.as_os_str().as_encoded_bytes())?;
        let directory = if index + 1 < parts.len() {
            libc::O_DIRECTORY
        } else {
            0
        };
        let fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK | directory,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("cannot safely open authorized material {relative}"));
        }
        current = unsafe { File::from_raw_fd(fd) };
    }
    Ok(current)
}

fn read_beneath(root: &Path, relative: &str, max: usize) -> Result<Vec<u8>> {
    let file = open_beneath(root, relative)?;
    if !file.metadata()?.is_file() {
        bail!("packet entry is not a regular file");
    }
    let mut bytes = Vec::new();
    file.take((max + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() > max {
        bail!("packet entry exceeds limits");
    }
    Ok(bytes)
}

fn directory_children(file: &File) -> Result<Vec<OsString>> {
    // Iterate the already-open directory, not its now-replaceable source pathname.
    let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let dir = unsafe { libc::fdopendir(fd) };
    if dir.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(std::io::Error::last_os_error().into());
    }
    let mut names = Vec::new();
    loop {
        // readdir uses errno to distinguish EOF from failure.
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().is_some_and(|code| code != 0) {
                unsafe {
                    libc::closedir(dir);
                }
                return Err(error.into());
            }
            break;
        }
        let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(OsString::from_vec(bytes.to_vec()));
        }
    }
    if unsafe { libc::closedir(dir) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(names)
}
