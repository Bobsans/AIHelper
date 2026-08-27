//! Downloading and unpacking the managed `psql`.
//!
//! The archive is fetched to the cache, hashed against a pinned SHA-256, and
//! unpacked with the path of every entry checked before it is written. A lock
//! file makes two `ah` processes take turns rather than write over each other,
//! and it is treated as stale after long enough that a crashed process cannot
//! block the next one forever.

use super::*;

pub(crate) fn download_managed_tool(
    version: &str,
    force: bool,
    timeout_secs: u64,
) -> Result<ToolDownloadOutput, InvocationResponse> {
    let download_started = Instant::now();
    let timeout_secs = timeout_secs.max(1);
    let manifest = match download_manifest(version) {
        Some(value) => value,
        None => {
            return Err(InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_UNSUPPORTED",
                format!(
                    "managed PostgreSQL tool download is not available for version '{}' on this platform",
                    version
                ),
            ));
        }
    };
    let cache_root = postgres_cache_root()?;
    let version_dir = cache_root.join(version);
    let bin_dir = version_dir.join("pgsql").join("bin");
    let psql_path = bin_dir.join(psql_exe_name());
    fs::create_dir_all(&cache_root).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!(
                "failed to create PostgreSQL tool cache '{}': {error}",
                cache_root.display()
            ),
        )
    })?;
    let _download_lock = acquire_download_lock(
        &cache_root,
        version,
        Duration::from_secs(timeout_secs.min(120)),
    )?;

    if psql_path.exists()
        && !force
        && let CandidateEvaluation::Accepted(_) =
            evaluate_candidate(ToolSource::ManagedCache, version_dir.clone())
    {
        return Ok(ToolDownloadOutput {
            command: "postgres.tool.download",
            version: version.to_owned(),
            url: manifest.url.to_owned(),
            sha256: manifest.sha256.to_owned(),
            cache_path: version_dir,
            bin_dir,
            psql_path,
            downloaded: false,
        });
    }

    let archive_path = cache_root.join(format!(
        "postgresql-{}-{}.zip.download",
        version, manifest.platform
    ));
    let remaining_timeout = Duration::from_secs(timeout_secs)
        .saturating_sub(download_started.elapsed())
        .max(Duration::from_secs(1));
    download_archive(
        manifest.url,
        manifest.sha256,
        &archive_path,
        remaining_timeout,
    )?;

    let temp_dir = cache_root.join(format!("{}.tmp.{}", version, std::process::id()));
    if temp_dir.exists() {
        remove_cache_dir(&cache_root, &temp_dir)?;
    }
    fs::create_dir_all(&temp_dir).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to create temporary extract directory '{}': {error}",
                temp_dir.display()
            ),
        )
    })?;
    extract_archive(&archive_path, &temp_dir)?;
    if !temp_dir
        .join("pgsql")
        .join("bin")
        .join(psql_exe_name())
        .is_file()
    {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "extracted archive did not contain pgsql/bin/{}",
                psql_exe_name()
            ),
        ));
    }
    if version_dir.exists() {
        remove_cache_dir(&cache_root, &version_dir)?;
    }
    fs::rename(&temp_dir, &version_dir).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to move extracted PostgreSQL tools into '{}': {error}",
                version_dir.display()
            ),
        )
    })?;
    let _ = fs::remove_file(&archive_path);

    Ok(ToolDownloadOutput {
        command: "postgres.tool.download",
        version: version.to_owned(),
        url: manifest.url.to_owned(),
        sha256: manifest.sha256.to_owned(),
        cache_path: version_dir,
        bin_dir,
        psql_path,
        downloaded: true,
    })
}

pub(crate) struct DownloadLock {
    pub(crate) path: PathBuf,
}

impl Drop for DownloadLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn acquire_download_lock(
    cache_root: &Path,
    version: &str,
    timeout: Duration,
) -> Result<DownloadLock, InvocationResponse> {
    let lock_path = cache_root.join(format!("{version}.lock"));
    let start = Instant::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "pid={}", std::process::id());
                return Ok(DownloadLock { path: lock_path });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&lock_path) {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                if start.elapsed() >= timeout {
                    return Err(InvocationResponse::error(
                        "POSTGRES_TOOL_DOWNLOAD_FAILED",
                        format!(
                            "timed out waiting for PostgreSQL tool download lock '{}'",
                            lock_path.display()
                        ),
                    ));
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(error) => {
                return Err(InvocationResponse::error(
                    "POSTGRES_TOOL_DOWNLOAD_FAILED",
                    format!(
                        "failed to create PostgreSQL tool download lock '{}': {error}",
                        lock_path.display()
                    ),
                ));
            }
        }
    }
}

pub(crate) fn lock_is_stale(lock_path: &Path) -> bool {
    fs::metadata(lock_path)
        .and_then(|metadata| metadata.modified())
        .and_then(|modified| {
            modified
                .elapsed()
                .map_err(|error| io::Error::other(error.to_string()))
        })
        .map(|age| age > Duration::from_secs(30 * 60))
        .unwrap_or(false)
}

pub(crate) struct DownloadManifest {
    pub(crate) url: &'static str,
    pub(crate) sha256: &'static str,
    pub(crate) platform: &'static str,
}

pub(crate) fn download_manifest(version: &str) -> Option<DownloadManifest> {
    if version == DEFAULT_POSTGRES_VERSION
        && cfg!(target_os = "windows")
        && cfg!(target_arch = "x86_64")
    {
        return Some(DownloadManifest {
            url: POSTGRES_18_4_WINDOWS_X64_URL,
            sha256: POSTGRES_18_4_WINDOWS_X64_SHA256,
            platform: "windows-x64",
        });
    }
    None
}

pub(crate) fn download_archive(
    url: &str,
    expected_sha256: &str,
    archive_path: &Path,
    timeout: Duration,
) -> Result<(), InvocationResponse> {
    let client = Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_FAILED",
                format!("failed to create HTTP client: {error}"),
            )
        })?;
    let mut response = client.get(url).send().map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!("failed to download PostgreSQL tool archive from '{url}': {error}"),
        )
    })?;
    if !response.status().is_success() {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!(
                "PostgreSQL tool archive download returned HTTP {} from '{}'",
                response.status(),
                url
            ),
        ));
    }
    let mut file = File::create(archive_path).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_DOWNLOAD_FAILED",
            format!(
                "failed to create download file '{}': {error}",
                archive_path.display()
            ),
        )
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response.read(&mut buffer).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_FAILED",
                format!("failed while reading PostgreSQL archive download: {error}"),
            )
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_DOWNLOAD_FAILED",
                format!("failed while writing '{}': {error}", archive_path.display()),
            )
        })?;
    }
    let actual = encode_lower_hex(hasher.finalize());
    if actual != expected_sha256 {
        let _ = fs::remove_file(archive_path);
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_CHECKSUM_FAILED",
            format!(
                "PostgreSQL tool archive checksum mismatch: expected {expected_sha256}, got {actual}"
            ),
        ));
    }
    Ok(())
}

/// `sha2` 0.11 digest output no longer implements `LowerHex`, so encode by hand.
pub(crate) fn encode_lower_hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn extract_archive(
    archive_path: &Path,
    dest_dir: &Path,
) -> Result<(), InvocationResponse> {
    let archive_file = File::open(archive_path).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to open archive '{}': {error}",
                archive_path.display()
            ),
        )
    })?;
    let mut archive = ZipArchive::new(archive_file).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_EXTRACT_FAILED",
            format!(
                "failed to read archive '{}': {error}",
                archive_path.display()
            ),
        )
    })?;

    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                format!("failed to read archive entry {index}: {error}"),
            )
        })?;
        let Some(enclosed_name) = file.enclosed_name() else {
            return Err(InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                "archive contains an unsafe path",
            ));
        };
        let outpath = dest_dir.join(enclosed_name);
        if file.is_dir() {
            fs::create_dir_all(&outpath).map_err(|error| {
                InvocationResponse::error(
                    "POSTGRES_TOOL_EXTRACT_FAILED",
                    format!(
                        "failed to create directory '{}': {error}",
                        outpath.display()
                    ),
                )
            })?;
            continue;
        }
        if let Some(parent) = outpath.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                InvocationResponse::error(
                    "POSTGRES_TOOL_EXTRACT_FAILED",
                    format!("failed to create directory '{}': {error}", parent.display()),
                )
            })?;
        }
        let mut outfile = File::create(&outpath).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                format!(
                    "failed to create extracted file '{}': {error}",
                    outpath.display()
                ),
            )
        })?;
        io::copy(&mut file, &mut outfile).map_err(|error| {
            InvocationResponse::error(
                "POSTGRES_TOOL_EXTRACT_FAILED",
                format!("failed to extract file '{}': {error}", outpath.display()),
            )
        })?;
    }
    Ok(())
}

pub(crate) fn remove_cache_dir(cache_root: &Path, target: &Path) -> Result<(), InvocationResponse> {
    let cache_root = fs::canonicalize(cache_root).unwrap_or_else(|_| cache_root.to_path_buf());
    let target_abs = if target.exists() {
        fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf())
    } else {
        target.to_path_buf()
    };
    if !target_abs.starts_with(&cache_root) {
        return Err(InvocationResponse::error(
            "POSTGRES_TOOL_CLEANUP_FAILED",
            format!(
                "refusing to remove path outside PostgreSQL tool cache: '{}'",
                target.display()
            ),
        ));
    }
    fs::remove_dir_all(target).map_err(|error| {
        InvocationResponse::error(
            "POSTGRES_TOOL_CLEANUP_FAILED",
            format!("failed to remove '{}': {error}", target.display()),
        )
    })
}
