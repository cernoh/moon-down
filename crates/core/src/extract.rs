use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Errors that surface as ExtractFailed on the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    PasswordProtected(String),
    NestedArchive(String),
    MissingBinary(String),
    Io(String),
    Unsupported(String),
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PasswordProtected(m) => write!(f, "{m}"),
            Self::NestedArchive(m) => write!(f, "{m}"),
            Self::MissingBinary(m) => write!(f, "{m}"),
            Self::Io(m) => write!(f, "{m}"),
            Self::Unsupported(m) => write!(f, "{m}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    Tar,
    TarGz,
    TarBz2,
    TarXz,
    SevenZ,
    Rar,
    Unknown,
}

const ARCHIVE_EXTS: &[&str] = &[
    ".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".7z", ".rar",
];

pub fn detect_kind(path: &Path) -> ArchiveKind {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_ascii_lowercase();
    if name.ends_with(".zip") {
        ArchiveKind::Zip
    } else if name.ends_with(".7z") {
        ArchiveKind::SevenZ
    } else if name.ends_with(".rar") {
        ArchiveKind::Rar
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        ArchiveKind::TarGz
    } else if name.ends_with(".tar.bz2") || name.ends_with(".tbz2") {
        ArchiveKind::TarBz2
    } else if name.ends_with(".tar.xz") || name.ends_with(".txz") {
        ArchiveKind::TarXz
    } else if name.ends_with(".tar") {
        ArchiveKind::Tar
    } else {
        ArchiveKind::Unknown
    }
}

pub fn is_archive_path(path: &Path) -> bool {
    let n = path.file_name().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
    ARCHIVE_EXTS.iter().any(|ext| n.ends_with(ext))
}

/// Find 7zz first, then 7z in PATH. Returns binary name to invoke.
pub fn find_7z_binary() -> Option<String> {
    find_7z_binary_in(&env_search_path())
}

/// Same lookup against an explicit colon-separated search path.
pub fn find_7z_binary_in(search: &str) -> Option<String> {
    for bin in ["7zz", "7z"] {
        if is_binary_in_path(bin, search) {
            return Some(bin.to_string());
        }
    }
    None
}

fn env_search_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

fn is_binary_in_path(bin: &str, path: &str) -> bool {
    {
        for dir in path.split(':') {
            let p = Path::new(dir).join(bin);
            if p.is_file() {
                // check executable bit on unix
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Ok(md) = fs::metadata(&p) {
                        if md.permissions().mode() & 0o111 != 0 {
                            return true;
                        }
                    }
                }
                #[cfg(not(unix))]
                return true;
            }
        }
    }
    false
}

fn is_encrypted_zip_error(e: &zip::result::ZipError) -> bool {
    match e {
        zip::result::ZipError::UnsupportedArchive(s) => {
            let l = s.to_ascii_lowercase();
            l.contains("password") || l.contains("encrypted")
        }
        _ => {
            let l = e.to_string().to_ascii_lowercase();
            l.contains("password") || l.contains("encrypted")
        }
    }
}

fn dest_for_archive(archive: &Path, base_dest: &Path, stem_subdir: bool) -> PathBuf {
    if !stem_subdir {
        return base_dest.to_path_buf();
    }
    let stem = archive
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("extracted");
    // for double extensions like .tar.gz, file_stem is "foo.tar", strip again
    let stem = if stem.ends_with(".tar") {
        &stem[..stem.len() - 4]
    } else {
        stem
    };
    base_dest.join(stem)
}

/// Extract a single archive to dest. No shell call for zip/tar family.
/// 7z/rar delegate to 7zz/7z binary.
pub fn extract_archive(archive: &Path, dest: &Path) -> Result<(), ExtractError> {
    extract_archive_in(archive, dest, &env_search_path())
}

/// Same as `extract_archive` with an explicit binary search path (used by tests and
/// by callers that resolve 7z themselves).
pub fn extract_archive_in(
    archive: &Path,
    dest: &Path,
    search: &str,
) -> Result<(), ExtractError> {
    let kind = detect_kind(archive);
    match kind {
        ArchiveKind::Zip => extract_zip(archive, dest),
        ArchiveKind::Tar | ArchiveKind::TarGz | ArchiveKind::TarBz2 | ArchiveKind::TarXz => {
            extract_tar_family(archive, dest, kind)
        }
        ArchiveKind::SevenZ | ArchiveKind::Rar => extract_via_7z_in(archive, dest, search),
        ArchiveKind::Unknown => Err(ExtractError::Unsupported(format!(
            "unsupported archive type: {}",
            archive.display()
        ))),
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), ExtractError> {
    let file = fs::File::open(archive).map_err(|e| ExtractError::Io(e.to_string()))?;
    let mut za = zip::ZipArchive::new(file).map_err(|e| {
        let s = e.to_string().to_ascii_lowercase();
        if s.contains("password") || s.contains("encrypted") {
            ExtractError::PasswordProtected(format!(
                "password-protected archive not supported: {} ({})",
                archive.display(),
                e
            ))
        } else {
            ExtractError::Io(e.to_string())
        }
    })?;

    // pre-check for password and nested
    for i in 0..za.len() {
        let entry = za.by_index(i).map_err(|e| {
            if is_encrypted_zip_error(&e) {
                ExtractError::PasswordProtected(format!(
                    "password-protected archive not supported: {}",
                    archive.display()
                ))
            } else {
                ExtractError::Io(e.to_string())
            }
        })?;
        // zip crate exposes encrypted flag via is_encrypted (method on ZipFile)
        // need to check via entry.is_encrypted() if available
        // also check name for nested
        let name = entry.name().to_ascii_lowercase();
        if ARCHIVE_EXTS.iter().any(|ext| name.ends_with(ext)) {
            return Err(ExtractError::NestedArchive(format!(
                "nested archives not supported: {} contains {}",
                archive.display(),
                entry.name()
            )));
        }
        // actual encrypted check via zip crate API: ZipFile::is_encrypted
        // Use unsafe downcast: try to call method if present via trait object not possible.
        // Instead, attempt to detect by trying to read with no password - zip crate returns error on encrypted
        // We'll check by inspecting central directory extra: simplest is to check if entry has extra field indicating encryption
        // Fallback: if extraction later fails with password string, we surface it.
    }

    fs::create_dir_all(dest).map_err(|e| ExtractError::Io(e.to_string()))?;

    for i in 0..za.len() {
        let mut entry = za.by_index(i).map_err(|e| {
            if is_encrypted_zip_error(&e) {
                ExtractError::PasswordProtected(format!(
                    "password-protected archive not supported: {}",
                    archive.display()
                ))
            } else {
                ExtractError::Io(e.to_string())
            }
        })?;
        // actual encrypted check: ZipFile::encrypted() or is_encrypted()
        // Use method if available via `entry.is_encrypted()` – zip 2.6 exposes it
        // Try via any:
        let is_enc = is_zip_entry_encrypted(&entry);
        if is_enc {
            return Err(ExtractError::PasswordProtected(format!(
                "password-protected archive not supported: {}",
                archive.display()
            )));
        }
        let out_path = dest.join(entry.mangled_name());
        // nested check on raw name already done; also check extracted path extension
        if is_archive_path(&out_path) {
            return Err(ExtractError::NestedArchive(format!(
                "nested archives not supported: {} contains {}",
                archive.display(),
                entry.name()
            )));
        }
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| ExtractError::Io(e.to_string()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent).map_err(|e| ExtractError::Io(e.to_string()))?;
            }
            let mut out = fs::File::create(&out_path).map_err(|e| ExtractError::Io(e.to_string()))?;
            std::io::copy(&mut entry, &mut out).map_err(|e| {
                let s = e.to_string().to_ascii_lowercase();
                if s.contains("password") || s.contains("encrypted") {
                    ExtractError::PasswordProtected(format!(
                        "password-protected archive not supported: {}",
                        archive.display()
                    ))
                } else {
                    ExtractError::Io(e.to_string())
                }
            })?;
        }
    }
    Ok(())
}

fn is_zip_entry_encrypted(entry: &zip::read::ZipFile) -> bool {
    entry.encrypted()
}

fn extract_tar_family(archive: &Path, dest: &Path, kind: ArchiveKind) -> Result<(), ExtractError> {
    let file = fs::File::open(archive).map_err(|e| ExtractError::Io(e.to_string()))?;
    fs::create_dir_all(dest).map_err(|e| ExtractError::Io(e.to_string()))?;

    match kind {
        ArchiveKind::Tar => do_tar_extract(file, dest, archive),
        ArchiveKind::TarGz => {
            let gz = flate2::read::GzDecoder::new(file);
            do_tar_extract(gz, dest, archive)
        }
        ArchiveKind::TarBz2 => {
            let bz = bzip2::read::BzDecoder::new(file);
            do_tar_extract(bz, dest, archive)
        }
        ArchiveKind::TarXz => {
            let xz = xz2::read::XzDecoder::new(file);
            do_tar_extract(xz, dest, archive)
        }
        _ => unreachable!(),
    }
}

fn do_tar_extract<R: std::io::Read>(reader: R, dest: &Path, archive: &Path) -> Result<(), ExtractError> {
    let mut ar = tar::Archive::new(reader);
    let entries = ar.entries().map_err(|e| ExtractError::Io(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| ExtractError::Io(e.to_string()))?;
        let path = entry.path().map_err(|e| ExtractError::Io(e.to_string()))?.to_path_buf();
        let name = path.to_string_lossy().to_ascii_lowercase();
        if ARCHIVE_EXTS.iter().any(|ext| name.ends_with(ext)) {
            return Err(ExtractError::NestedArchive(format!(
                "nested archives not supported: {} contains {}",
                archive.display(),
                path.display()
            )));
        }
        // also check for password? tar has no password
        entry.unpack_in(dest).map_err(|e| ExtractError::Io(e.to_string()))?;
    }
    Ok(())
}

#[allow(dead_code)]
fn extract_via_7z(archive: &Path, dest: &Path) -> Result<(), ExtractError> {
    extract_via_7z_in(archive, dest, &env_search_path())
}

fn extract_via_7z_in(archive: &Path, dest: &Path, search: &str) -> Result<(), ExtractError> {
    let bin = find_7z_binary_in(search).ok_or_else(|| {
        ExtractError::MissingBinary(
            "7z/rar extraction requires 7zz or 7z (p7zip) — install p7zip-full or 7zip and ensure 7zz or 7z is in PATH".into(),
        )
    })?;
    fs::create_dir_all(dest).map_err(|e| ExtractError::Io(e.to_string()))?;
    let out = Command::new(&bin)
        .arg("x")
        .arg(format!("-o{}", dest.display()))
        .arg("-y")
        .arg("-p") // empty password; if archive needs password, 7z will error
        .arg(archive)
        .output()
        .map_err(|e| ExtractError::Io(e.to_string()))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
    let stderr = String::from_utf8_lossy(&out.stderr).to_ascii_lowercase();
    let combined = format!("{stdout} {stderr}");

    if combined.contains("wrong password")
        || combined.contains("enter password")
        || combined.contains("password")
            && (combined.contains("encrypted") || combined.contains("can not open"))
        || combined.contains("data error") && combined.contains("password")
    {
        // 7z often says "Wrong password" or "Enter password"
        if combined.contains("password") || combined.contains("wrong") {
            return Err(ExtractError::PasswordProtected(format!(
                "password-protected archive not supported: {}",
                archive.display()
            )));
        }
    }
    // Also detect password via exit code + message containing password
    if !out.status.success() {
        if combined.contains("password") || combined.contains("wrong") || combined.contains("encrypted") {
            return Err(ExtractError::PasswordProtected(format!(
                "password-protected archive not supported: {}",
                archive.display()
            )));
        }
        return Err(ExtractError::Io(format!(
            "7z failed for {}: {}",
            archive.display(),
            combined.trim()
        )));
    }

    // nested check after extraction: scan dest for archive files
    if let Ok(entries) = scan_for_archives(dest) {
        if !entries.is_empty() {
            return Err(ExtractError::NestedArchive(format!(
                "nested archives not supported: {} contains {}",
                archive.display(),
                entries[0].display()
            )));
        }
    }

    Ok(())
}

fn scan_for_archives(dir: &Path) -> Result<Vec<PathBuf>, ExtractError> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = match fs::read_dir(&d) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if is_archive_path(&p) {
                found.push(p);
            }
        }
    }
    Ok(found)
}

/// Extract all archives belonging to a package. `archives` are absolute paths to archive files.
/// `base_dest` is the package target dir. When `stem_subdir` is true each archive extracts into base_dest/<stem>/.
pub fn extract_many(
    archives: &[PathBuf],
    base_dest: &Path,
    stem_subdir: bool,
) -> Result<(), ExtractError> {
    for archive in archives {
        let dest = dest_for_archive(archive, base_dest, stem_subdir);
        extract_archive(archive, &dest)?;
        // after each archive, verify no nested archives were produced (zip/tar already checked during, 7z after)
        // for zip/tar we already errored on nested entry; for completeness scan dest
        if let Ok(nested) = scan_for_archives(&dest) {
            // only error if nested found that wasn't the archive itself (we already scanned)
            // But scan includes only extracted files; if archive contained archive, we already errored.
            // For tar/zip we errored earlier, so this is just safety for any missed case.
            if !nested.is_empty() {
                // check that nested file is not the archive itself (archive lives outside dest)
                return Err(ExtractError::NestedArchive(format!(
                    "nested archives not supported: {} contains {}",
                    archive.display(),
                    nested[0].display()
                )));
            }
        }
    }
    Ok(())
}

/// Discover archive files in a directory (non-recursive top-level). Used to find package archives.
pub fn discover_archives(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && is_archive_path(&p) {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detect_kind_cases() {
        assert_eq!(detect_kind(Path::new("a.zip")), ArchiveKind::Zip);
        assert_eq!(detect_kind(Path::new("a.tar")), ArchiveKind::Tar);
        assert_eq!(detect_kind(Path::new("a.tar.gz")), ArchiveKind::TarGz);
        assert_eq!(detect_kind(Path::new("a.tgz")), ArchiveKind::TarGz);
        assert_eq!(detect_kind(Path::new("a.tar.bz2")), ArchiveKind::TarBz2);
        assert_eq!(detect_kind(Path::new("a.tar.xz")), ArchiveKind::TarXz);
        assert_eq!(detect_kind(Path::new("a.7z")), ArchiveKind::SevenZ);
        assert_eq!(detect_kind(Path::new("a.rar")), ArchiveKind::Rar);
        assert_eq!(detect_kind(Path::new("a.txt")), ArchiveKind::Unknown);
    }

    #[test]
    fn fin_7z_prefers_7zz() {
        // just check function runs and never returns that binary
        if let Some(bin) = find_7z_binary() {
            assert!(bin == "7zz" || bin == "7z");
            assert_ne!(bin, ["un", "rar"].concat());
        }
    }

    #[test]
    fn zip_round_trip_no_shell() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("hello.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("hello.txt", opts).unwrap();
            zw.write_all(b"hello world").unwrap();
            zw.finish().unwrap();
        }
        let dest = dir.path().join("out");
        extract_archive(&zip_path, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("hello.txt")).unwrap(), "hello world");
        // ensure no forbidden binary is spawned; tar/zip path uses pure Rust
        let src = include_str!("extract.rs");
        let forbidden = ["un", "rar"].concat();
        assert!(!src.contains(&forbidden), "must never call forbidden binary");
    }

    #[test]
    fn tar_gz_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let tar_path = dir.path().join("arc.tar.gz");
        {
            let f = fs::File::create(&tar_path).unwrap();
            let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            let mut tar = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "hi.txt", b"hello" as &[u8]).unwrap();
            tar.finish().unwrap();
        }
        let dest = dir.path().join("out");
        extract_archive(&tar_path, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("hi.txt")).unwrap(), "hello");
    }

    #[test]
    fn nested_zip_fails() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("nested.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("inner.zip", opts).unwrap();
            zw.write_all(b"not a real zip but name triggers nested").unwrap();
            zw.finish().unwrap();
        }
        let dest = dir.path().join("out");
        let err = extract_archive(&zip_path, &dest).unwrap_err();
        assert!(matches!(err, ExtractError::NestedArchive(_)));
        assert!(err.to_string().to_ascii_lowercase().contains("nested"));
    }

    #[test]
    fn missing_binary_hint() {
        // Force missing with an explicit search path: mutating PATH here would
        // race sibling tests that spawn real binaries.
        let dir = tempfile::tempdir().unwrap();
        let rar_path = dir.path().join("a.rar");
        fs::write(&rar_path, b"dummy").unwrap();
        let dest = dir.path().join("out");
        let err = extract_via_7z_in(&rar_path, &dest, "/nonexistent").unwrap_err();
        assert!(matches!(err, ExtractError::MissingBinary(_)));
        assert!(err.to_string().contains("7zz") || err.to_string().contains("7z"));
        assert!(err.to_string().to_ascii_lowercase().contains("install"));
    }

    #[test]
    fn no_forbidden_binary_in_source() {
        let src = include_str!("extract.rs");
        let forbidden = ["un", "rar"].concat();
        assert!(!src.contains(&forbidden), "must never call forbidden binary");
    }

    #[test]
    fn password_zip_fails() {
        use zip::unstable::write::FileOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("enc.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored)
                .with_deprecated_encryption(b"secret");
            zw.start_file("secret.txt", opts).unwrap();
            zw.write_all(b"hidden").unwrap();
            zw.finish().unwrap();
        }
        let dest = dir.path().join("out");
        let err = extract_archive(&zip_path, &dest).unwrap_err();
        assert!(matches!(err, ExtractError::PasswordProtected(_)), "got {:?}", err);
        assert!(err.to_string().to_ascii_lowercase().contains("password"));
    }

    #[test]
    fn extract_many_stem_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("myarc.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("a.txt", opts).unwrap();
            zw.write_all(b"a").unwrap();
            zw.finish().unwrap();
        }
        let base = dir.path().join("dest");
        extract_many(&[zip_path.clone()], &base, true).unwrap();
        assert!(base.join("myarc").join("a.txt").exists());
        // without stem subdir
        let base2 = dir.path().join("dest2");
        extract_many(&[zip_path], &base2, false).unwrap();
        assert!(base2.join("a.txt").exists());
    }

    #[test]
    fn tar_bz2_and_xz_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        // bz2
        let bz_path = dir.path().join("arc.tar.bz2");
        {
            let f = fs::File::create(&bz_path).unwrap();
            let bz = bzip2::write::BzEncoder::new(f, bzip2::Compression::default());
            let mut tar = tar::Builder::new(bz);
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, "b.txt", b"bye" as &[u8]).unwrap();
            tar.finish().unwrap();
        }
        let dest = dir.path().join("out_bz");
        extract_archive(&bz_path, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("b.txt")).unwrap(), "bye");
        // xz
        let xz_path = dir.path().join("arc.tar.xz");
        {
            let f = fs::File::create(&xz_path).unwrap();
            let xz = xz2::write::XzEncoder::new(f, 6);
            let mut tar = tar::Builder::new(xz);
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, "c.txt", b"cee" as &[u8]).unwrap();
            tar.finish().unwrap();
        }
        let dest2 = dir.path().join("out_xz");
        extract_archive(&xz_path, &dest2).unwrap();
        assert_eq!(fs::read_to_string(dest2.join("c.txt")).unwrap(), "cee");
    }
}
