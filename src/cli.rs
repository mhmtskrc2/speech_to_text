//! Headless mode, used when the program is given arguments.
//!
//! The GUI is the main interface; this exists so the same pipeline can be
//! scripted, piped, or checked from a terminal.

use std::path::PathBuf;
use std::sync::mpsc::channel;

use anyhow::{Result, bail};

use crate::job::{self, Event, Input};
use crate::models::{self, MODELS};
use crate::transcribe::{self, OutputFormat};

const USAGE: &str = "\
Sesten Metne (whisper.cpp)

KULLANIM:
    whisper_gui                          grafik arayüzü açar
    whisper_gui <url|dosya> [seçenekler] metne dönüştürüp yazar

SEÇENEKLER:
    -m, --model <ad>     model (varsayılan: large-v3). --list-models ile listele
    -l, --lang <kod>     konuşma dili, örn. tr / en. Belirtilmezse otomatik algılanır
    -f, --format <biçim> lines | plain | timestamps | srt  (varsayılan: lines)
    -o, --output <yol>   çıktı dosyası. Belirtilmezse stdout'a yazar
    -t, --threads <n>    iş parçacığı sayısı
        --translate      konuşmayı İngilizceye çevirir
        --prompt <metin> sözlük ipucu (özel isimler, terimler)
        --no-gpu         GPU hızlandırmayı kapatır
        --list-models    kullanılabilir modelleri listeler
    -h, --help           bu yardımı gösterir
";

struct Args {
    input: Input,
    model_index: usize,
    language: Option<String>,
    format: OutputFormat,
    output: Option<PathBuf>,
    threads: i32,
    translate: bool,
    prompt: String,
    use_gpu: bool,
}

/// Runs the headless pipeline. Returns the process exit code.
pub fn main(argv: Vec<String>) -> i32 {
    match run(argv) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("hata: {err:#}");
            1
        }
    }
}

fn run(argv: Vec<String>) -> Result<i32> {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        let _ = out(USAGE);
        return Ok(0);
    }
    if argv.iter().any(|a| a == "--list-models") {
        list_models();
        return Ok(0);
    }

    let args = parse(argv)?;
    let (sender, receiver) = channel();

    job::spawn(
        job::Request {
            input: args.input,
            model_index: args.model_index,
            language: args.language,
            translate: args.translate,
            threads: args.threads,
            initial_prompt: args.prompt,
            use_gpu: args.use_gpu,
        },
        sender,
    );

    // Progress goes to stderr so stdout stays a clean transcript.
    let mut last_progress = -1i32;
    for event in receiver {
        match event {
            Event::Stage(stage) => {
                last_progress = -1;
                eprintln!("[{}]", stage.label());
            }
            Event::Status(text) => eprintln!("  {text}"),
            Event::Progress(Some(value)) => {
                let percent = (value * 100.0).round() as i32;
                if percent / 10 != last_progress / 10 {
                    last_progress = percent;
                    eprintln!("  %{percent}");
                }
            }
            Event::Progress(None) | Event::Segment(_) => {}
            Event::Finished {
                segments,
                audio_secs,
                elapsed_secs,
                transcribe_secs,
                decoder,
                ..
            } => {
                eprintln!(
                    "[bitti] {} parça · {audio_secs:.0} sn ses · toplam {elapsed_secs:.1} sn · \
                     metne dönüştürme {transcribe_secs:.1} sn ({:.1}× gerçek zaman) · {decoder}",
                    segments.len(),
                    if transcribe_secs > 0.0 { audio_secs / transcribe_secs } else { 0.0 }
                );
                let text = transcribe::render(&segments, args.format);
                match &args.output {
                    Some(path) => {
                        std::fs::write(path, text.as_bytes())?;
                        eprintln!("[kaydedildi] {}", path.display());
                    }
                    None => {
                        return match out(&text) {
                            Ok(()) => Ok(0),
                            // A closed pipe is a normal end, not a failure.
                            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(0),
                            Err(e) => Err(e.into()),
                        };
                    }
                }
                return Ok(0);
            }
            Event::Failed(message) => {
                eprintln!("hata: {message}");
                return Ok(1);
            }
            Event::Cancelled => return Ok(130),
        }
    }

    bail!("işlem beklenmedik şekilde sona erdi");
}

fn list_models() {
    for model in MODELS {
        let line = format!(
            "{:<22} {:>8}  {}{}\n",
            model.id,
            model.size_label(),
            if model.is_downloaded() {
                "[indirildi] "
            } else {
                ""
            },
            model.label
        );
        if out(&line).is_err() {
            return;
        }
    }
}

/// Writes to stdout, reporting a closed pipe instead of panicking.
///
/// Rust ignores `SIGPIPE`, so a plain `println!` panics once the reader goes
/// away — and `whisper_gui ... | head` is an ordinary thing to do.
fn out(text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()
}

fn parse(argv: Vec<String>) -> Result<Args> {
    let mut input: Option<String> = None;
    let mut model = MODELS[models::DEFAULT_MODEL].id.to_string();
    let mut language = None;
    let mut format = OutputFormat::Lines;
    let mut output = None;
    let mut threads = transcribe::default_threads();
    let mut translate = false;
    let mut prompt = String::new();
    let mut use_gpu = true;

    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| -> Result<String> {
            iter.next()
                .ok_or_else(|| anyhow::anyhow!("{name} seçeneği bir değer bekliyor"))
        };

        match arg.as_str() {
            "-m" | "--model" => model = value("--model")?,
            "-l" | "--lang" | "--language" => language = Some(value("--lang")?),
            "-o" | "--output" => output = Some(PathBuf::from(value("--output")?)),
            "-t" | "--threads" => threads = value("--threads")?.parse()?,
            "--prompt" => prompt = value("--prompt")?,
            "-f" | "--format" => {
                format = match value("--format")?.as_str() {
                    "lines" | "satirlar" | "satırlar" => OutputFormat::Lines,
                    "plain" | "txt" => OutputFormat::Plain,
                    "timestamps" | "ts" => OutputFormat::Timestamped,
                    "srt" => OutputFormat::Srt,
                    other => {
                        bail!("bilinmeyen biçim: {other} (lines | plain | timestamps | srt)")
                    }
                }
            }
            "--translate" => translate = true,
            "--no-gpu" => use_gpu = false,
            other if other.starts_with('-') => bail!("bilinmeyen seçenek: {other}"),
            other => input = Some(other.to_string()),
        }
    }

    let input = input.ok_or_else(|| anyhow::anyhow!("bir ses adresi veya dosya yolu verin"))?;
    let model_index = MODELS
        .iter()
        .position(|m| m.id == model)
        .ok_or_else(|| anyhow::anyhow!("bilinmeyen model: {model} (--list-models ile bakın)"))?;

    let lower = input.to_ascii_lowercase();
    let input = if lower.starts_with("http://") || lower.starts_with("https://") {
        Input::Url(input)
    } else {
        Input::File(PathBuf::from(input))
    };

    // Touch the catalog so a typo'd model name fails before any download.
    let _ = models::MODELS[model_index].file_name();

    Ok(Args {
        input,
        model_index,
        language,
        format,
        output,
        threads,
        translate,
        prompt,
        use_gpu,
    })
}
