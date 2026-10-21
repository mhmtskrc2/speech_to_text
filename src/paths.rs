//! Platform-appropriate cache locations.
//!
//! Everything the app downloads (models, ffmpeg, temporary audio) lives under a
//! single cache directory so that a user can reclaim the space by deleting one
//! folder:
//!
//! * macOS:   `~/Library/Caches/dev.whisper-gui.WhisperGui`
//! * Windows: `%LOCALAPPDATA%\whisper-gui\WhisperGui\cache`
//! * Linux:   `~/.cache/whispergui`

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use directories::ProjectDirs;

fn cache_root() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "whisper-gui", "WhisperGui")
        .context("işletim sisteminin önbellek dizini bulunamadı")?;
    Ok(dirs.cache_dir().to_path_buf())
}

fn ensure(path: PathBuf) -> Result<PathBuf> {
    fs::create_dir_all(&path)
        .with_context(|| format!("dizin oluşturulamadı: {}", path.display()))?;
    Ok(path)
}

/// Downloaded ggml model files.
pub fn models_dir() -> Result<PathBuf> {
    ensure(cache_root()?.join("models"))
}

/// Auto-downloaded helper binaries (ffmpeg).
pub fn tools_dir() -> Result<PathBuf> {
    ensure(cache_root()?.join("tools"))
}

/// Scratch space for fetched audio and decoded PCM.
pub fn work_dir() -> Result<PathBuf> {
    ensure(cache_root()?.join("work"))
}

/// Shown in the UI so the user knows what to delete to free disk space.
pub fn cache_root_for_display() -> String {
    cache_root()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "(bilinmiyor)".to_string())
}
