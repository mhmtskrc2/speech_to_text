//! Turning an arbitrary audio file into what whisper.cpp expects:
//! mono 32-bit float PCM at 16 kHz.
//!
//! MP3 (and wav/flac/m4a/ogg) are decoded in-process with symphonia, which keeps
//! the common case dependency-free. Anything symphonia can't open — exotic
//! containers, video files, unusual codecs — falls back to ffmpeg, which is
//! fetched on demand by [`crate::ffmpeg`].

use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::ffmpeg;
use crate::net::Cancelled;

/// The only sample rate whisper models accept.
pub const TARGET_RATE: u32 = 16_000;

/// Audio shorter than this can't be transcribed by whisper.
const MIN_SAMPLES: usize = TARGET_RATE as usize / 10;

pub struct Decoded {
    pub samples: Vec<f32>,
    /// Which decoder produced the samples, for the status line.
    pub via: &'static str,
}

impl Decoded {
    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / TARGET_RATE as f64
    }
}

/// Decodes `path` to mono 16 kHz, trying the built-in decoders first.
pub fn load_mono_16k(
    path: &Path,
    cancel: &Arc<AtomicBool>,
    status: &mut dyn FnMut(String),
    progress: &mut dyn FnMut(Option<f32>),
) -> Result<Decoded> {
    match decode_builtin(path, cancel, progress) {
        Ok((samples, rate)) => {
            let samples = finish(samples, rate, status)?;
            Ok(Decoded {
                samples,
                via: "symphonia",
            })
        }
        Err(err) if crate::net::is_cancelled(&err) => Err(err),
        Err(err) => {
            status(format!(
                "Dahili çözümleyici bu dosyayı açamadı ({err}). ffmpeg deneniyor…"
            ));

            let mut download_progress = |done: u64, total: Option<u64>| {
                progress(total.map(|t| done as f32 / t as f32));
            };
            let source = ffmpeg::ensure(cancel, status, &mut download_progress)?;
            let tool = source.path().to_path_buf();
            match &source {
                ffmpeg::Source::Path(p) => status(format!("ffmpeg kullanılıyor: {}", p.display())),
                ffmpeg::Source::Cached(_) => status("Önbellekteki ffmpeg kullanılıyor.".into()),
                ffmpeg::Source::Downloaded(p) => {
                    status(format!("ffmpeg indirildi: {}", p.display()))
                }
            }

            let duration = ffmpeg::probe_duration(&tool, path);
            status("Ses çözümleniyor (ffmpeg)…".into());
            let samples = ffmpeg::decode_to_mono_16k(&tool, path, duration, cancel, progress)?;
            check_length(&samples)?;
            Ok(Decoded {
                samples,
                via: "ffmpeg",
            })
        }
    }
}

/// Downmixes to mono but keeps the native sample rate; resampling happens once,
/// in [`finish`].
fn decode_builtin(
    path: &Path,
    cancel: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(Option<f32>),
) -> Result<(Vec<f32>, u32)> {
    let file = File::open(path).with_context(|| format!("dosya açılamadı: {}", path.display()))?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .context("ses biçimi tanınamadı")?;

    let (track_id, params, total_frames) = reader
        .tracks()
        .iter()
        .find_map(|track| match track.codec_params.as_ref() {
            Some(CodecParameters::Audio(audio)) => {
                Some((track.id, audio.clone(), track.num_frames.filter(|n| *n > 0)))
            }
            _ => None,
        })
        .ok_or_else(|| anyhow!("dosyada ses akışı bulunamadı"))?;

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .context("bu ses kodeki için çözümleyici yok")?;

    let mut rate = params.sample_rate.unwrap_or(0);
    let mut mono: Vec<f32> = Vec::new();
    let mut interleaved: Vec<f32> = Vec::new();
    let mut reported = 0u64;

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!(Cancelled));
        }

        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // A truncated stream is common for URLs; keep whatever we decoded.
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(err) => return Err(err).context("ses akışı okunamadı"),
        };

        if packet.track_id != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(buffer) => {
                append_mono(&buffer, &mut interleaved, &mut mono, &mut rate);
            }
            // Recoverable per-packet glitches: skip the packet, keep going.
            Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::ResetRequired) => continue,
            Err(err) => return Err(err).context("ses çözümlenemedi"),
        }

        // Report at most once per percent, so a long file doesn't flood the UI.
        match total_frames {
            Some(total) if mono.len() as u64 - reported > total / 100 => {
                reported = mono.len() as u64;
                progress(Some((reported as f32 / total as f32).clamp(0.0, 1.0)));
            }
            None => progress(None),
            _ => {}
        }
    }

    if mono.is_empty() {
        bail!("ses verisi çözümlenemedi");
    }
    if rate == 0 {
        bail!("ses örnekleme hızı belirlenemedi");
    }

    Ok((mono, rate))
}

fn append_mono(
    buffer: &GenericAudioBufferRef<'_>,
    scratch: &mut Vec<f32>,
    out: &mut Vec<f32>,
    rate: &mut u32,
) {
    let spec = buffer.spec();
    *rate = spec.rate();
    let channels = spec.channels().count().max(1);

    buffer.copy_to_vec_interleaved(scratch);

    if channels == 1 {
        out.append(scratch);
        return;
    }

    out.reserve(scratch.len() / channels);
    for frame in scratch.chunks_exact(channels) {
        out.push(frame.iter().sum::<f32>() / channels as f32);
    }
}

fn finish(samples: Vec<f32>, rate: u32, status: &mut dyn FnMut(String)) -> Result<Vec<f32>> {
    let samples = if rate == TARGET_RATE {
        samples
    } else {
        status(format!("{rate} Hz'den {TARGET_RATE} Hz'e dönüştürülüyor…"));
        resample(&samples, rate, TARGET_RATE)?
    };
    check_length(&samples)?;
    Ok(samples)
}

fn check_length(samples: &[f32]) -> Result<()> {
    if samples.len() < MIN_SAMPLES {
        bail!(
            "ses çok kısa ({:.2} saniye); konuşma çözümlenemez",
            samples.len() as f64 / TARGET_RATE as f64
        );
    }
    Ok(())
}

/// Band-limited sample rate conversion of a single mono channel.
fn resample(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>> {
    const CHUNK: usize = 1024;
    const CHANNELS: usize = 1;

    let frames_in = input.len();
    let mut resampler = Fft::<f32>::new(
        from as usize,
        to as usize,
        CHUNK,
        CHANNELS,
        FixedSync::Input,
    )
    .context("yeniden örnekleyici oluşturulamadı")?;

    let delay = resampler.output_delay();
    let expected = (frames_in as f64 * to as f64 / from as f64).round() as usize;
    // Room for the filter delay plus the padding the final partial chunk emits.
    let mut output = vec![0f32; expected + delay + resampler.output_frames_max() + CHUNK];

    let input_adapter = InterleavedSlice::new(input, CHANNELS, frames_in)
        .map_err(|e| anyhow!("giriş arabelleği hazırlanamadı: {e}"))?;
    let capacity = output.len();
    let mut output_adapter = InterleavedSlice::new_mut(&mut output, CHANNELS, capacity)
        .map_err(|e| anyhow!("çıkış arabelleği hazırlanamadı: {e}"))?;

    let mut indexing = Indexing::new();
    let mut left = frames_in;
    let mut next = resampler.input_frames_next();

    while left >= next {
        let (consumed, produced) = resampler
            .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
            .map_err(|e| anyhow!("yeniden örnekleme başarısız: {e}"))?;
        indexing.input_offset += consumed;
        indexing.output_offset += produced;
        left -= consumed;
        next = resampler.input_frames_next();
    }

    // Flush the tail, telling the resampler that only `left` frames are real.
    indexing.partial_len = Some(left);
    resampler
        .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
        .map_err(|e| anyhow!("yeniden örnekleme tamamlanamadı: {e}"))?;

    // Drop the leading filter delay, then the padding past the real length.
    let start = delay.min(output.len());
    output.drain(..start);
    output.truncate(expected.min(output.len()));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{TARGET_RATE, resample};

    /// A 440 Hz tone must survive 44.1 kHz -> 16 kHz with its amplitude and
    /// length intact; a broken resampler shows up as silence or wrong length.
    #[test]
    fn resamples_a_tone() {
        let from = 44_100;
        let seconds = 1.0;
        let input: Vec<f32> = (0..(from as f64 * seconds) as usize)
            .map(|n| {
                (std::f64::consts::TAU * 440.0 * n as f64 / from as f64).sin() as f32 * 0.5
            })
            .collect();

        let out = resample(&input, from, TARGET_RATE).unwrap();

        let expected = TARGET_RATE as usize;
        assert!(
            out.len().abs_diff(expected) <= 2,
            "beklenen ~{expected} örnek, gelen {}",
            out.len()
        );

        // Skip the edges, where the window taper legitimately reduces amplitude.
        let middle = &out[2_000..expected - 2_000];
        let peak = middle.iter().fold(0f32, |acc, s| acc.max(s.abs()));
        assert!((peak - 0.5).abs() < 0.05, "tepe genlik {peak}, 0.5 olmalıydı");
    }

    #[test]
    fn passes_through_matching_rate() {
        let input = vec![0.1f32; 100];
        assert_eq!(resample(&input, 16_000, 16_000).unwrap().len(), 100);
    }
}
