//! ffmpeg fallback for audio the built-in Rust decoders can't handle.
//!
//! Resolution order: an `ffmpeg` already on `PATH`, then a copy downloaded by a
//! previous run, then a one-off download of a pinned static build. The pinned
//! size and SHA-256 of every asset are baked in below, so the binary that gets
//! executed is always exactly the reviewed one.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};

use crate::audio::TARGET_RATE;
use crate::net::{self, Cancelled, Expect};
use crate::paths;

const RELEASE: &str = "b6.0";
const BASE_URL: &str = "https://github.com/eugeneware/ffmpeg-static/releases/download";

struct Asset {
    name: &'static str,
    gz_size: u64,
    gz_sha256: &'static str,
}

/// The static build matching the host platform, if we have one pinned.
fn asset() -> Option<Asset> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    match (os, arch) {
        ("macos", "aarch64") => Some(Asset {
            name: "ffmpeg-darwin-arm64",
            gz_size: 19_253_088,
            gz_sha256: "6be74d6f449889c2e87a75873894f8520cad56c08ac76f2a628d85b0519daaca",
        }),
        ("macos", "x86_64") => Some(Asset {
            name: "ffmpeg-darwin-x64",
            gz_size: 25_067_704,
            gz_sha256: "a12354fce7eb62361473bbe10d53a1893695babd35869ec8e92e5dfea8d0440b",
        }),
        ("windows", "x86_64") => Some(Asset {
            name: "ffmpeg-win32-x64",
            gz_size: 28_894_894,
            gz_sha256: "450d66226c79405c724e821f291cab0911e934bfa9fa2231adcab587f3e07b50",
        }),
        ("linux", "x86_64") => Some(Asset {
            name: "ffmpeg-linux-x64",
            gz_size: 28_928_723,
            gz_sha256: "17c1ae10b52ac499180679fe6ba77e17642390c4eedb0f1e3b0ac045da55128f",
        }),
        ("linux", "aarch64") => Some(Asset {
            name: "ffmpeg-linux-arm64",
            gz_size: 25_131_803,
            gz_sha256: "2b708b2d15041d2a192c1db24c7a8a1d24f645a8242dce1c744ff2392b86ada1",
        }),
        _ => None,
    }
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

fn cached_binary() -> Result<PathBuf> {
    Ok(paths::tools_dir()?.join(binary_name()))
}

fn runs(path: &Path) -> bool {
    command(path)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Builds a `Command` that never pops up a console window on Windows.
fn command(program: &Path) -> Command {
    #[allow(unused_mut)] // only Windows needs to mutate it
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Where ffmpeg came from, so the UI can tell the user what happened.
pub enum Source {
    Path(PathBuf),
    Cached(PathBuf),
    Downloaded(PathBuf),
}

impl Source {
    pub fn path(&self) -> &Path {
        match self {
            Source::Path(p) | Source::Cached(p) | Source::Downloaded(p) => p,
        }
    }
}

/// Returns a usable ffmpeg, downloading one on first need.
pub fn ensure(
    cancel: &Arc<AtomicBool>,
    status: &mut dyn FnMut(String),
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<Source> {
    let on_path = PathBuf::from(binary_name());
    if runs(&on_path) {
        return Ok(Source::Path(on_path));
    }

    let cached = cached_binary()?;
    if runs(&cached) {
        return Ok(Source::Cached(cached));
    }

    let asset = asset().ok_or_else(|| {
        anyhow!(
            "bu platform için ({} {}) hazır ffmpeg paketi yok; lütfen ffmpeg'i kurup PATH'e ekleyin",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;

    status(format!(
        "ffmpeg bulunamadı, indiriliyor ({})…",
        crate::models::human_size(asset.gz_size)
    ));

    let archive = paths::tools_dir()?.join(format!("{}.gz", asset.name));
    net::download_to_file(
        &format!("{BASE_URL}/{RELEASE}/{}.gz", asset.name),
        &archive,
        Expect {
            size: Some(asset.gz_size),
            sha256: Some(asset.gz_sha256),
            max_size: None,
        },
        cancel,
        progress,
    )
    .context("ffmpeg indirilemedi")?;

    status("ffmpeg açılıyor…".to_string());
    unpack(&archive, &cached)?;
    let _ = std::fs::remove_file(&archive);

    if !runs(&cached) {
        bail!("indirilen ffmpeg çalıştırılamadı: {}", cached.display());
    }

    Ok(Source::Downloaded(cached))
}

fn unpack(archive: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(archive)
        .with_context(|| format!("arşiv açılamadı: {}", archive.display()))?;
    let mut decoder = flate2::read::GzDecoder::new(file);
    let temp = dest.with_extension("part");

    {
        let mut out = std::fs::File::create(&temp)
            .with_context(|| format!("dosya oluşturulamadı: {}", temp.display()))?;
        std::io::copy(&mut decoder, &mut out).context("ffmpeg arşivi açılamadı")?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))
            .context("ffmpeg çalıştırma izni verilemedi")?;
    }

    std::fs::rename(&temp, dest)
        .with_context(|| format!("ffmpeg yerleştirilemedi: {}", dest.display()))?;
    Ok(())
}

/// Decodes any ffmpeg-supported input into mono 32-bit float PCM at 16 kHz —
/// exactly the format whisper wants, so no further resampling is needed.
///
/// PCM goes to a temporary file rather than a pipe: ffmpeg's progress lines are
/// read from stderr while it runs, and draining two pipes at once is how you
/// deadlock.
pub fn decode_to_mono_16k(
    ffmpeg: &Path,
    input: &Path,
    duration_hint: Option<f64>,
    cancel: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(Option<f32>),
) -> Result<Vec<f32>> {
    let raw = paths::work_dir()?.join("decoded.f32le");
    let _ = std::fs::remove_file(&raw);

    let mut child = command(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
        .arg("-i")
        .arg(input)
        .args([
            "-vn",
            "-map",
            "a:0",
            "-ac",
            "1",
            "-ar",
            &TARGET_RATE.to_string(),
            "-f",
            "f32le",
            "-progress",
            "pipe:2",
        ])
        .arg(&raw)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("ffmpeg başlatılamadı: {}", ffmpeg.display()))?;

    let stderr = child.stderr.take().expect("stderr piped");
    let mut messages = Vec::new();

    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };

        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&raw);
            return Err(anyhow!(Cancelled));
        }

        // `-progress pipe:2` emits `key=value` lines; anything else is a log line.
        if let Some(micros) = line.strip_prefix("out_time_ms=") {
            if let (Ok(micros), Some(total)) = (micros.trim().parse::<f64>(), duration_hint)
                && total > 0.0
            {
                progress(Some(((micros / 1_000_000.0) / total).clamp(0.0, 1.0) as f32));
            } else {
                progress(None);
            }
        } else if !line.contains('=') && !line.trim().is_empty() {
            messages.push(line);
        }
    }

    let status = child.wait().context("ffmpeg beklenirken hata oluştu")?;
    if !status.success() {
        let _ = std::fs::remove_file(&raw);
        let detail = if messages.is_empty() {
            String::new()
        } else {
            format!("\n{}", messages.join("\n"))
        };
        bail!("ffmpeg ses çözümlemesi başarısız oldu{detail}");
    }

    let samples = read_f32le(&raw)?;
    let _ = std::fs::remove_file(&raw);

    if samples.is_empty() {
        bail!("ffmpeg ses verisi üretemedi; dosyada ses akışı olmayabilir");
    }
    Ok(samples)
}

fn read_f32le(path: &Path) -> Result<Vec<f32>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("çözümlenen ses okunamadı: {}", path.display()))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0) as usize;
    let mut reader = BufReader::new(file);
    let mut out = Vec::with_capacity(len / 4);
    let mut chunk = [0u8; 4096];

    loop {
        let mut filled = 0;
        // f32 samples must not be split across reads, so top the buffer up to a
        // multiple of 4 before converting.
        while filled < chunk.len() {
            let n = reader.read(&mut chunk[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        for bytes in chunk[..filled].as_chunks::<4>().0 {
            out.push(f32::from_le_bytes(*bytes));
        }
        if filled < chunk.len() {
            break;
        }
    }

    Ok(out)
}

/// Reads the media duration in seconds, used only to turn ffmpeg's progress
/// into a percentage.
pub fn probe_duration(ffmpeg: &Path, input: &Path) -> Option<f64> {
    let output = command(ffmpeg)
        .args(["-hide_banner", "-nostdin"])
        .arg("-i")
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;

    let text = String::from_utf8_lossy(&output.stderr);
    let rest = text.split("Duration: ").nth(1)?;
    let stamp = rest.split(',').next()?.trim();
    parse_timestamp(stamp)
}

fn parse_timestamp(stamp: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in stamp.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    (seconds > 0.0).then_some(seconds)
}

#[cfg(test)]
mod tests {
    use super::parse_timestamp;

    #[test]
    fn parses_hms() {
        assert_eq!(parse_timestamp("00:01:30.50"), Some(90.5));
        assert_eq!(parse_timestamp("01:00:00.00"), Some(3600.0));
        assert_eq!(parse_timestamp("N/A"), None);
    }
}
