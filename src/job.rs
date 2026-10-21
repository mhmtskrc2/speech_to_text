//! The worker pipeline: fetch → ensure model → decode → transcribe.
//!
//! Runs on its own thread and reports everything back to the UI through a
//! channel, so the interface never blocks and stays cancellable.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Instant;

use anyhow::{Result, anyhow, bail};

use crate::audio;
use crate::models;
use crate::net::{self, Expect};
use crate::paths;
use crate::transcribe::{self, Segment};

/// Refuse absurdly large audio downloads rather than filling the disk.
const MAX_AUDIO_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    FetchingAudio,
    PreparingModel,
    DecodingAudio,
    Transcribing,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::FetchingAudio => "Ses indiriliyor",
            Stage::PreparingModel => "Model hazırlanıyor",
            Stage::DecodingAudio => "Ses çözümleniyor",
            Stage::Transcribing => "Metne dönüştürülüyor",
        }
    }
}

/// Messages the worker sends to the UI.
pub enum Event {
    Stage(Stage),
    Status(String),
    /// `None` means "working, but progress is unknown".
    Progress(Option<f32>),
    Segment(Segment),
    Finished {
        segments: Vec<Segment>,
        audio_secs: f64,
        elapsed_secs: f64,
        decoder: &'static str,
        suggested_stem: String,
    },
    Failed(String),
    Cancelled,
}

/// What the UI collected from the user.
pub struct Request {
    /// A URL to download, or a local file already on disk.
    pub input: Input,
    pub model_index: usize,
    pub language: Option<String>,
    pub translate: bool,
    pub threads: i32,
    pub initial_prompt: String,
    pub use_gpu: bool,
}

pub enum Input {
    Url(String),
    File(PathBuf),
}

/// Spawns the pipeline. The returned flag cancels it when set.
pub fn spawn(request: Request, events: Sender<Event>) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);

    std::thread::Builder::new()
        .name("transcribe".into())
        .spawn(move || {
            let result = execute(&request, &worker_cancel, &events);
            let message = match result {
                Ok(()) => return,
                Err(err) if net::is_cancelled(&err) => {
                    let _ = events.send(Event::Cancelled);
                    return;
                }
                Err(err) => format_error(&err),
            };
            let _ = events.send(Event::Failed(message));
        })
        .expect("worker thread spawn");

    cancel
}

/// Flattens the `anyhow` context chain into something readable in the UI.
fn format_error(err: &anyhow::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(current) = source {
        let text = current.to_string();
        if !parts.iter().any(|p| p == &text) {
            parts.push(text);
        }
        source = current.source();
    }
    parts.join("\n  nedeni: ")
}

fn execute(request: &Request, cancel: &Arc<AtomicBool>, events: &Sender<Event>) -> Result<()> {
    let started = Instant::now();
    let model = models::MODELS
        .get(request.model_index)
        .ok_or_else(|| anyhow!("geçersiz model seçimi"))?;

    let status = |text: String| {
        let _ = events.send(Event::Status(text));
    };

    // 1. Get the audio onto disk.
    let (audio_path, suggested_stem) = match &request.input {
        Input::Url(url) => {
            let url = url.trim();
            validate_url(url)?;
            let _ = events.send(Event::Stage(Stage::FetchingAudio));
            status(format!("İndiriliyor: {url}"));

            let stem = net::file_stem_from_url(url).unwrap_or_else(|| "transkript".to_string());
            let target = paths::work_dir()?.join("input.audio");
            let mut progress = |done: u64, total: Option<u64>| {
                let _ = events.send(Event::Progress(
                    total.map(|total| done as f32 / total as f32),
                ));
                let _ = events.send(Event::Status(match total {
                    Some(total) => format!(
                        "İndiriliyor: {} / {}",
                        models::human_size(done),
                        models::human_size(total)
                    ),
                    None => format!("İndiriliyor: {}", models::human_size(done)),
                }));
            };

            net::download_to_file(
                url,
                &target,
                Expect {
                    size: None,
                    sha256: None,
                    max_size: Some(MAX_AUDIO_BYTES),
                },
                cancel,
                &mut progress,
            )?;
            (target, stem)
        }
        Input::File(path) => {
            if !path.is_file() {
                bail!("dosya bulunamadı: {}", path.display());
            }
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "transkript".to_string());
            (path.clone(), stem)
        }
    };

    check_cancelled(cancel)?;

    // 2. Fetch the model if this is the first time it's been used.
    let _ = events.send(Event::Stage(Stage::PreparingModel));
    if model.is_downloaded() {
        status(format!("Model hazır: {}", model.file_name()));
        let _ = events.send(Event::Progress(Some(1.0)));
    } else {
        status(format!(
            "{} modeli indiriliyor ({})…",
            model.id,
            model.size_label()
        ));
    }

    let mut model_status = |text: String| {
        let _ = events.send(Event::Status(text));
    };
    let mut model_progress = |done: u64, total: Option<u64>| {
        let _ = events.send(Event::Progress(
            total.map(|total| done as f32 / total as f32),
        ));
        let _ = events.send(Event::Status(format!(
            "Model: {} / {}",
            models::human_size(done),
            models::human_size(total.unwrap_or(model.size))
        )));
    };
    let model_path =
        models::ensure_available(model, cancel, &mut model_status, &mut model_progress)?;

    check_cancelled(cancel)?;

    // 3. Decode to mono 16 kHz.
    let _ = events.send(Event::Stage(Stage::DecodingAudio));
    let _ = events.send(Event::Progress(None));
    status("Ses çözümleniyor…".to_string());

    let mut decode_status = |text: String| {
        let _ = events.send(Event::Status(text));
    };
    let mut decode_progress = |fraction: Option<f32>| {
        let _ = events.send(Event::Progress(fraction));
    };
    let decoded = audio::load_mono_16k(
        &audio_path,
        cancel,
        &mut decode_status,
        &mut decode_progress,
    )?;

    check_cancelled(cancel)?;

    let audio_secs = decoded.duration_secs();
    let _ = events.send(Event::Status(format!(
        "{} saniyelik ses hazır ({})",
        audio_secs.round() as i64,
        decoded.via
    )));

    // 4. Transcribe.
    let _ = events.send(Event::Stage(Stage::Transcribing));
    let _ = events.send(Event::Progress(Some(0.0)));

    let progress_events = events.clone();
    let segment_events = events.clone();
    let options = transcribe::Options {
        model_path,
        language: request.language.clone(),
        translate: request.translate,
        threads: request.threads,
        initial_prompt: Some(request.initial_prompt.clone()).filter(|p| !p.trim().is_empty()),
        use_gpu: request.use_gpu,
    };

    let segments = transcribe::run(
        &decoded.samples,
        &options,
        cancel,
        Box::new(move |percent| {
            let _ = progress_events.send(Event::Progress(Some(percent as f32 / 100.0)));
        }),
        Box::new(move |segment| {
            let _ = segment_events.send(Event::Segment(segment));
        }),
    )
    .map_err(|err| {
        // A model we had already verified no longer loads: forget that verdict so
        // the next attempt re-checks the file and re-downloads it if needed.
        if err.downcast_ref::<transcribe::ModelLoadFailed>().is_some() {
            models::forget_verification(model);
            err.context("model dosyası bozulmuş olabilir; tekrar denediğinizde yeniden indirilecek")
        } else {
            err
        }
    })?;

    if segments.is_empty() {
        bail!("ses içinde konuşma bulunamadı");
    }

    let _ = events.send(Event::Finished {
        segments,
        audio_secs,
        elapsed_secs: started.elapsed().as_secs_f64(),
        decoder: decoded.via,
        suggested_stem,
    });

    Ok(())
}

fn check_cancelled(cancel: &Arc<AtomicBool>) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(anyhow!(net::Cancelled))
    } else {
        Ok(())
    }
}

fn validate_url(url: &str) -> Result<()> {
    if url.is_empty() {
        bail!("bir ses dosyası adresi girin");
    }
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        bail!("adres http:// veya https:// ile başlamalı");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_url;

    #[test]
    fn accepts_http_urls() {
        assert!(validate_url("https://example.com/a.mp3").is_ok());
        assert!(validate_url("HTTP://example.com/a.mp3").is_ok());
    }

    #[test]
    fn rejects_other_schemes_and_blanks() {
        assert!(validate_url("").is_err());
        assert!(validate_url("file:///etc/passwd").is_err());
        assert!(validate_url("example.com/a.mp3").is_err());
    }
}
