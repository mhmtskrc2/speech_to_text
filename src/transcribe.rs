//! whisper.cpp wrapper: loads a ggml model and turns 16 kHz mono PCM into
//! timestamped segments.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::net::Cancelled;

/// Marker attached when the ggml file itself couldn't be opened, so the caller
/// can drop its cached verification and re-download next time.
#[derive(Debug)]
pub struct ModelLoadFailed;

impl std::fmt::Display for ModelLoadFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("model dosyası açılamadı")
    }
}

impl std::error::Error for ModelLoadFailed {}

#[derive(Clone, Debug)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub struct Options {
    pub model_path: PathBuf,
    /// ISO code such as `"tr"`; `None` lets whisper detect the language.
    pub language: Option<String>,
    /// Translate the speech to English instead of transcribing verbatim.
    pub translate: bool,
    pub threads: i32,
    /// Vocabulary hint (names, jargon) that nudges the decoder.
    pub initial_prompt: Option<String>,
    pub use_gpu: bool,
}

/// Progress is reported as 0–100 by whisper.cpp.
pub type ProgressCallback = Box<dyn FnMut(i32) + 'static>;
/// Called for each segment as soon as it is decoded, for live output.
pub type SegmentCallback = Box<dyn FnMut(Segment) + 'static>;

/// Which GPU backend, if any, is compiled into this build.
///
/// Metal is wired in for every macOS target in `Cargo.toml`, so it is a
/// property of the platform rather than of a feature flag — checking a feature
/// here would report "no GPU" on a Mac that is in fact using one. CUDA and
/// Vulkan stay opt-in because they need an SDK at build time.
pub const GPU_BACKEND: Option<&str> = if cfg!(target_os = "macos") {
    Some("Metal")
} else if cfg!(feature = "cuda") {
    Some("CUDA")
} else if cfg!(feature = "vulkan") {
    Some("Vulkan")
} else {
    None
};

/// Default thread count: leave one core for the UI, but always use at least one.
pub fn default_threads() -> i32 {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    cores.saturating_sub(1).clamp(1, 16) as i32
}

pub fn run(
    samples: &[f32],
    options: &Options,
    cancel: &Arc<AtomicBool>,
    mut on_progress: ProgressCallback,
    on_segment: SegmentCallback,
) -> Result<Vec<Segment>> {
    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow!(Cancelled));
    }

    let context_params = WhisperContextParameters {
        use_gpu: options.use_gpu,
        ..Default::default()
    };

    let path = options
        .model_path
        .to_str()
        .ok_or_else(|| anyhow!("model yolu UTF-8 değil: {}", options.model_path.display()))?;

    let context = WhisperContext::new_with_params(path, context_params)
        .map_err(|err| anyhow!("{err}").context(ModelLoadFailed))
        .with_context(|| format!("model yüklenemedi: {path}"))?;

    if let Some(language) = options.language.as_deref()
        && language != "en"
        && !context.is_multilingual()
    {
        bail!(
            "seçilen model yalnızca İngilizce konuşmayı tanıyabilir; çok dilli bir model seçin"
        );
    }

    let mut state = context
        .create_state()
        .context("whisper durumu oluşturulamadı")?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(options.threads.max(1));
    params.set_translate(options.translate);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);

    // `language` has to outlive `params`, which borrows it.
    let language = options.language.clone();
    params.set_language(language.as_deref().or(Some("auto")));

    if let Some(prompt) = options.initial_prompt.as_deref().filter(|p| !p.is_empty()) {
        params.set_initial_prompt(prompt);
    }

    params.set_progress_callback_safe(move |percent: i32| on_progress(percent));
    params.set_segment_callback_safe_lossy(wrap_segment_callback(on_segment));

    // `set_abort_callback_safe` is broken in whisper-rs 0.16: it stores a
    // `Box<Box<dyn FnMut() -> bool>>` as user data but instantiates the
    // trampoline with the concrete closure type, so the callback reinterprets a
    // fat pointer as the closure and returns garbage — which whisper reads as
    // "abort", failing every encode with error -6. Drive the raw C callback
    // ourselves instead; `cancel` outlives the `full()` call below, so the
    // pointer stays valid for exactly as long as whisper can use it.
    unsafe {
        params.set_abort_callback(Some(abort_trampoline));
        params.set_abort_callback_user_data(Arc::as_ptr(cancel) as *mut c_void);
    }

    state
        .full(params, samples)
        .context("ses metne dönüştürülemedi")?;

    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow!(Cancelled));
    }

    let mut segments = Vec::with_capacity(state.full_n_segments().max(0) as usize);
    for segment in state.as_iter() {
        segments.push(Segment {
            start_ms: centis_to_ms(segment.start_timestamp()),
            end_ms: centis_to_ms(segment.end_timestamp()),
            text: segment.to_str_lossy().unwrap_or_default().trim().to_string(),
        });
    }
    segments.retain(|s| !s.text.is_empty());

    Ok(segments)
}

/// Returning `true` tells whisper.cpp to stop as soon as it can.
///
/// # Safety
/// `user_data` must point to a live `AtomicBool`; see the call site.
unsafe extern "C" fn abort_trampoline(user_data: *mut c_void) -> bool {
    debug_assert!(!user_data.is_null());
    unsafe { &*(user_data as *const AtomicBool) }.load(Ordering::Relaxed)
}

fn wrap_segment_callback(
    mut on_segment: SegmentCallback,
) -> impl FnMut(whisper_rs::SegmentCallbackData) + 'static {
    move |data| {
        let text = data.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        on_segment(Segment {
            start_ms: centis_to_ms(data.start_timestamp),
            end_ms: centis_to_ms(data.end_timestamp),
            text,
        });
    }
}

/// whisper.cpp reports timestamps in centiseconds.
fn centis_to_ms(centis: i64) -> i64 {
    centis * 10
}

/// `hh:mm:ss,mmm`, matching the SRT convention.
pub fn format_timestamp(ms: i64) -> String {
    let ms = ms.max(0);
    let (hours, rest) = (ms / 3_600_000, ms % 3_600_000);
    let (minutes, rest) = (rest / 60_000, rest % 60_000);
    let (seconds, millis) = (rest / 1_000, rest % 1_000);
    format!("{hours:02}:{minutes:02}:{seconds:02},{millis:03}")
}

/// The output formats offered in the UI.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// One paragraph of plain text.
    Plain,
    /// `[hh:mm:ss,mmm --> hh:mm:ss,mmm] text` per line.
    Timestamped,
    /// Standard SubRip subtitles.
    Srt,
}

impl OutputFormat {
    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::Plain => "Düz metin",
            OutputFormat::Timestamped => "Zaman damgalı metin",
            OutputFormat::Srt => "SRT altyazı",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Plain | OutputFormat::Timestamped => "txt",
            OutputFormat::Srt => "srt",
        }
    }

    pub const ALL: [OutputFormat; 3] = [
        OutputFormat::Plain,
        OutputFormat::Timestamped,
        OutputFormat::Srt,
    ];
}

pub fn render(segments: &[Segment], format: OutputFormat) -> String {
    match format {
        OutputFormat::Plain => {
            let text = segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let mut out = text.trim().to_string();
            if !out.is_empty() {
                out.push('\n');
            }
            out
        }
        OutputFormat::Timestamped => {
            let mut out = String::new();
            for segment in segments {
                out.push_str(&format!(
                    "[{} --> {}] {}\n",
                    format_timestamp(segment.start_ms),
                    format_timestamp(segment.end_ms),
                    segment.text
                ));
            }
            out
        }
        OutputFormat::Srt => {
            let mut out = String::new();
            for (index, segment) in segments.iter().enumerate() {
                out.push_str(&format!(
                    "{}\n{} --> {}\n{}\n\n",
                    index + 1,
                    format_timestamp(segment.start_ms),
                    format_timestamp(segment.end_ms),
                    segment.text
                ));
            }
            out
        }
    }
}

/// Index of English in [`LANGUAGES`]; English-only models are pinned to it.
pub fn english_index() -> usize {
    LANGUAGES
        .iter()
        .position(|(_, code)| *code == Some("en"))
        .unwrap_or(0)
}

/// Languages offered in the picker. `None` means auto-detect.
pub const LANGUAGES: &[(&str, Option<&str>)] = &[
    ("Otomatik algıla", None),
    ("Türkçe", Some("tr")),
    ("İngilizce", Some("en")),
    ("Almanca", Some("de")),
    ("Fransızca", Some("fr")),
    ("İspanyolca", Some("es")),
    ("İtalyanca", Some("it")),
    ("Rusça", Some("ru")),
    ("Arapça", Some("ar")),
    ("Azerbaycan Türkçesi", Some("az")),
    ("Hollandaca", Some("nl")),
    ("Portekizce", Some("pt")),
    ("Çince", Some("zh")),
    ("Japonca", Some("ja")),
    ("Korece", Some("ko")),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Segment> {
        vec![
            Segment {
                start_ms: 0,
                end_ms: 1_500,
                text: "Merhaba dünya".into(),
            },
            Segment {
                start_ms: 1_500,
                end_ms: 3_250,
                text: "ikinci cümle".into(),
            },
        ]
    }

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_timestamp(0), "00:00:00,000");
        assert_eq!(format_timestamp(3_661_001), "01:01:01,001");
    }

    #[test]
    fn renders_plain_text() {
        assert_eq!(
            render(&sample(), OutputFormat::Plain),
            "Merhaba dünya ikinci cümle\n"
        );
    }

    #[test]
    fn renders_srt() {
        let srt = render(&sample(), OutputFormat::Srt);
        assert!(srt.starts_with("1\n00:00:00,000 --> 00:00:01,500\nMerhaba dünya\n\n"));
        assert!(srt.contains("2\n00:00:01,500 --> 00:00:03,250\n"));
    }

    #[test]
    fn empty_plain_output_has_no_stray_newline() {
        assert_eq!(render(&[], OutputFormat::Plain), "");
    }
}
