//! The ggml model catalog, plus lazy download-on-first-use.
//!
//! Sizes and digests were taken from the upstream `ggerganov/whisper.cpp`
//! Hugging Face repository (the `x-linked-size` / `x-linked-etag` of each LFS
//! object), so a download that doesn't match byte-for-byte is rejected.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};

use crate::net::{self, Expect, ProgressFn};
use crate::paths;

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

pub struct Model {
    /// Matches the `ggml-<id>.bin` filename upstream.
    pub id: &'static str,
    pub label: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// English-only models ignore the language setting.
    pub multilingual: bool,
}

impl Model {
    pub fn file_name(&self) -> String {
        format!("ggml-{}.bin", self.id)
    }

    pub fn url(&self) -> String {
        format!("{BASE_URL}/{}", self.file_name())
    }

    pub fn path(&self) -> Result<PathBuf> {
        Ok(paths::models_dir()?.join(self.file_name()))
    }

    /// Cheap check used to render the catalog; only compares the size, since
    /// hashing a 1.6 GB file on every repaint is not an option.
    pub fn is_downloaded(&self) -> bool {
        self.path()
            .and_then(|p| Ok(std::fs::metadata(p)?.len() == self.size))
            .unwrap_or(false)
    }

    pub fn size_label(&self) -> String {
        human_size(self.size)
    }
}

/// Ordered roughly from fastest to most accurate.
pub const MODELS: &[Model] = &[
    Model {
        id: "tiny-q5_1",
        label: "tiny (q5_1) — en hızlı, en düşük doğruluk",
        size: 32_152_673,
        sha256: "818710568da3ca15689e31a743197b520007872ff9576237bda97bd1b469c3d7",
        multilingual: true,
    },
    Model {
        id: "tiny",
        label: "tiny — hızlı",
        size: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        multilingual: true,
    },
    Model {
        id: "base-q5_1",
        label: "base (q5_1) — dengeli, küçük indirme",
        size: 59_707_625,
        sha256: "422f1ae452ade6f30a004d7e5c6a43195e4433bc370bf23fac9cc591f01a8898",
        multilingual: true,
    },
    Model {
        id: "base",
        label: "base — dengeli (önerilen başlangıç)",
        size: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        multilingual: true,
    },
    Model {
        id: "base.en",
        label: "base.en — yalnızca İngilizce",
        size: 147_964_211,
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        multilingual: false,
    },
    Model {
        id: "small-q5_1",
        label: "small (q5_1) — iyi doğruluk, orta boy",
        size: 190_085_487,
        sha256: "ae85e4a935d7a567bd102fe55afc16bb595bdb618e11b2fc7591bc08120411bb",
        multilingual: true,
    },
    Model {
        id: "small",
        label: "small — iyi doğruluk",
        size: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        multilingual: true,
    },
    Model {
        id: "medium-q5_0",
        label: "medium (q5_0) — yüksek doğruluk",
        size: 539_212_467,
        sha256: "19fea4b380c3a618ec4723c3eef2eb785ffba0d0538cf43f8f235e7b3b34220f",
        multilingual: true,
    },
    Model {
        id: "medium",
        label: "medium — yüksek doğruluk, yavaş",
        size: 1_533_763_059,
        sha256: "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208",
        multilingual: true,
    },
    Model {
        id: "large-v3-turbo-q5_0",
        label: "large-v3-turbo (q5_0) — en iyi doğruluk/hız (önerilen)",
        size: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        multilingual: true,
    },
    Model {
        id: "large-v3-turbo",
        label: "large-v3-turbo — en iyi doğruluk/hız",
        size: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        multilingual: true,
    },
    Model {
        id: "large-v3",
        label: "large-v3 — en yüksek doğruluk, en yavaş",
        size: 3_095_033_483,
        sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
        multilingual: true,
    },
];

/// Index of the model selected the first time the app runs.
pub const DEFAULT_MODEL: usize = 3; // base

/// Returns the local path to `model`, downloading it on first use.
///
/// A copy whose size matches is only trusted once its SHA-256 has been checked.
/// The result is remembered in a small marker file next to the model, so the
/// (slow) hashing happens once rather than on every launch — a model that was
/// hand-placed from elsewhere gets validated, and a corrupt one is replaced
/// instead of being handed to whisper as a confusing load failure.
pub fn ensure_available(
    model: &Model,
    cancel: &Arc<AtomicBool>,
    status: &mut dyn FnMut(String),
    progress: ProgressFn<'_>,
) -> Result<PathBuf> {
    let path = model.path()?;
    let marker = marker_path(&path);

    if file_len(&path) == Some(model.size) {
        if marker_matches(&marker, model.sha256) {
            return Ok(path);
        }

        status(format!("{} modeli doğrulanıyor…", model.id));
        let mut verify_progress = |fraction: f32| {
            progress((fraction * model.size as f32) as u64, Some(model.size));
        };
        if net::verify_file(&path, model.sha256, &mut verify_progress)? {
            write_marker(&marker, model.sha256);
            return Ok(path);
        }

        status(format!(
            "{} modeli bozuk görünüyor, yeniden indiriliyor…",
            model.id
        ));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&marker);
    }

    net::download_to_file(
        &model.url(),
        &path,
        Expect {
            size: Some(model.size),
            sha256: Some(model.sha256),
            max_size: None,
        },
        cancel,
        progress,
    )
    .with_context(|| format!("{} modeli indirilemedi", model.id))?;

    write_marker(&marker, model.sha256);
    Ok(path)
}

/// Drops the cached "this copy is good" marker, so the next run re-verifies the
/// file. Called when whisper refuses to load a model we previously trusted.
pub fn forget_verification(model: &Model) {
    if let Ok(path) = model.path() {
        let _ = std::fs::remove_file(marker_path(&path));
    }
}

fn file_len(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path).map(|m| m.len()).ok()
}

fn marker_path(model_path: &std::path::Path) -> PathBuf {
    let mut name = model_path.file_name().unwrap_or_default().to_os_string();
    name.push(".sha256");
    model_path.with_file_name(name)
}

fn marker_matches(marker: &std::path::Path, sha256: &str) -> bool {
    std::fs::read_to_string(marker)
        .map(|recorded| recorded.trim().eq_ignore_ascii_case(sha256))
        .unwrap_or(false)
}

fn write_marker(marker: &std::path::Path, sha256: &str) {
    // Losing the marker only costs a re-verification, so failures are ignored.
    let _ = std::fs::write(marker, sha256);
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}
