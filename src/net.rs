//! Streaming downloads with progress reporting, integrity checks and cancellation.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

/// Marker error so callers can tell "user pressed Cancel" apart from a failure.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("işlem iptal edildi")
    }
}

impl std::error::Error for Cancelled {}

pub fn is_cancelled(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Cancelled>().is_some()
}

/// What the caller expects the downloaded bytes to be.
#[derive(Clone, Copy, Default)]
pub struct Expect<'a> {
    /// Exact byte count; mismatches are rejected.
    pub size: Option<u64>,
    /// Lowercase hex SHA-256 digest.
    pub sha256: Option<&'a str>,
    /// Upper bound used when the exact size is unknown, to avoid filling the disk.
    pub max_size: Option<u64>,
}

/// `(bytes_done, total_bytes_if_known)`
pub type ProgressFn<'a> = &'a mut dyn FnMut(u64, Option<u64>);

/// Downloads `url` to `dest`, atomically: bytes land in a sibling `.part` file
/// that is only renamed into place once the integrity checks pass. A partial or
/// corrupt download therefore never masquerades as a usable file.
pub fn download_to_file(
    url: &str,
    dest: &Path,
    expect: Expect<'_>,
    cancel: &Arc<AtomicBool>,
    progress: ProgressFn<'_>,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow!(Cancelled));
    }

    let mut response = ureq::get(url)
        .call()
        .with_context(|| format!("indirme isteği başarısız: {url}"))?;

    let status = response.status();
    if !status.is_success() {
        bail!("sunucu {status} döndü: {url}");
    }

    let total = response
        .body()
        .content_length()
        .or(expect.size)
        .filter(|n| *n > 0);

    if let (Some(expected), Some(actual)) = (expect.size, total)
        && expected != actual
    {
        bail!(
            "beklenmeyen dosya boyutu: {actual} bayt geldi, {expected} bayt bekleniyordu ({url})"
        );
    }

    let limit = expect.size.or(expect.max_size);
    if let (Some(limit), Some(total)) = (limit, total)
        && total > limit
    {
        bail!(
            "dosya izin verilen boyuttan büyük: {} > {} bayt",
            total,
            limit
        );
    }

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("dizin oluşturulamadı: {}", parent.display()))?;
    }
    let part = part_path(dest);

    let mut reader = response.body_mut().as_reader();
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    let mut buf = vec![0u8; 256 * 1024];

    {
        let file = File::create(&part)
            .with_context(|| format!("dosya oluşturulamadı: {}", part.display()))?;
        let mut writer = BufWriter::new(file);

        loop {
            if cancel.load(Ordering::Relaxed) {
                drop(writer);
                let _ = fs::remove_file(&part);
                return Err(anyhow!(Cancelled));
            }

            let n = reader
                .read(&mut buf)
                .with_context(|| format!("indirme sırasında okuma hatası: {url}"))?;
            if n == 0 {
                break;
            }

            done += n as u64;
            if let Some(limit) = limit
                && done > limit
            {
                drop(writer);
                let _ = fs::remove_file(&part);
                bail!("dosya izin verilen {limit} bayt sınırını aştı: {url}");
            }

            hasher.update(&buf[..n]);
            writer
                .write_all(&buf[..n])
                .with_context(|| format!("diske yazılamadı: {}", part.display()))?;
            progress(done, total);
        }

        writer
            .flush()
            .with_context(|| format!("diske yazılamadı: {}", part.display()))?;
    }

    let verify = |ok: bool, msg: String| -> Result<()> {
        if ok {
            Ok(())
        } else {
            let _ = fs::remove_file(&part);
            Err(anyhow!(msg))
        }
    };

    if let Some(expected) = expect.size {
        verify(
            done == expected,
            format!("indirme yarım kaldı: {done}/{expected} bayt ({url})"),
        )?;
    }

    if let Some(expected) = expect.sha256 {
        let actual = hex(&hasher.finalize());
        verify(
            actual.eq_ignore_ascii_case(expected),
            format!(
                "SHA-256 doğrulaması başarısız ({url})\nbeklenen: {expected}\ngelen:    {actual}"
            ),
        )?;
    }

    fs::rename(&part, dest).with_context(|| {
        format!(
            "indirilen dosya taşınamadı: {} -> {}",
            part.display(),
            dest.display()
        )
    })?;

    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Hashes an existing file and compares it against `sha256`.
///
/// `progress` is called with the fraction read, since this walks the whole file
/// and a large model takes a noticeable moment.
pub fn verify_file(path: &Path, sha256: &str, progress: &mut dyn FnMut(f32)) -> Result<bool> {
    let mut file =
        File::open(path).with_context(|| format!("dosya açılamadı: {}", path.display()))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0).max(1);

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done: u64 = 0;

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done as f32 / total as f32);
    }

    Ok(hex(&hasher.finalize()).eq_ignore_ascii_case(sha256))
}

/// Best-effort filename for a URL, used to name the downloaded audio and the
/// default `.txt` output.
pub fn file_stem_from_url(url: &str) -> Option<String> {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let path = without_scheme.split(['?', '#']).next()?;
    let last = path.rsplit('/').find(|s| !s.is_empty())?;
    let stem = Path::new(last).file_stem()?.to_string_lossy().to_string();
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}
