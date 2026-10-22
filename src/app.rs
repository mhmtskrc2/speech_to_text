//! The egui front end.
//!
//! All the slow work happens on a worker thread (see [`crate::job`]); this file
//! only draws state and drains the event channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use crate::job::{self, Event, Input, Stage};
use crate::models::{self, MODELS};
use crate::paths;
use crate::transcribe::{self, OutputFormat, Segment};

pub struct App {
    url: String,
    local_file: Option<PathBuf>,
    model_index: usize,
    language_index: usize,
    format: OutputFormat,
    translate: bool,
    threads: i32,
    prompt: String,

    auto_save: bool,
    output_dir: Option<PathBuf>,

    segments: Vec<Segment>,
    /// Edited text shown in the output pane; re-rendered when the format changes.
    text: String,
    suggested_stem: String,
    saved_to: Option<PathBuf>,

    stage: Option<Stage>,
    status: String,
    progress: Option<f32>,
    /// When the current stage began, so the UI can show elapsed time and an
    /// estimate. Without these, a slow transcription is indistinguishable from
    /// a hung one.
    stage_started: Option<Instant>,
    /// Progress at the moment the stage started; the estimate is based on how
    /// much has been covered since, not on the absolute percentage.
    stage_start_progress: f32,
    error: Option<String>,
    summary: Option<String>,

    cancel: Option<Arc<AtomicBool>>,
    events: Option<Receiver<Event>>,
    sender: Option<Sender<Event>>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            url: String::new(),
            local_file: None,
            model_index: models::DEFAULT_MODEL,
            language_index: 0,
            format: OutputFormat::Lines,
            translate: false,
            threads: transcribe::default_threads(),
            prompt: String::new(),
            auto_save: true,
            output_dir: default_output_dir(),
            segments: Vec::new(),
            text: String::new(),
            suggested_stem: "transkript".into(),
            saved_to: None,
            stage: None,
            status: "Bir ses adresi girip \"Metne Dönüştür\"e basın.".into(),
            progress: None,
            stage_started: None,
            stage_start_progress: 0.0,
            error: None,
            summary: None,
            cancel: None,
            events: None,
            sender: None,
        }
    }
}

fn default_output_dir() -> Option<PathBuf> {
    let dirs = directories::UserDirs::new()?;
    dirs.download_dir()
        .or_else(|| dirs.document_dir())
        .map(|p| p.to_path_buf())
        .or_else(|| Some(dirs.home_dir().to_path_buf()))
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::default()
    }

    fn is_running(&self) -> bool {
        self.events.is_some()
    }

    fn start(&mut self) {
        if self.is_running() {
            return;
        }

        let input = match (&self.local_file, self.url.trim()) {
            (Some(path), _) => Input::File(path.clone()),
            (None, url) => Input::Url(url.to_string()),
        };

        self.segments.clear();
        self.text.clear();
        self.error = None;
        self.summary = None;
        self.saved_to = None;
        self.progress = None;
        self.stage = None;
        self.stage_started = None;
        self.stage_start_progress = 0.0;
        self.status = "Başlatılıyor…".into();

        let language = transcribe::LANGUAGES
            .get(self.language_index)
            .and_then(|(_, code)| *code)
            .map(|code| code.to_string());

        let (sender, receiver) = channel();
        let cancel = job::spawn(
            job::Request {
                input,
                model_index: self.model_index,
                language,
                translate: self.translate,
                threads: self.threads,
                initial_prompt: self.prompt.clone(),
                // Arayüzde tercih yok: hızlandırma varsa kullanılır.
                use_gpu: true,
            },
            sender.clone(),
        );

        self.cancel = Some(cancel);
        self.events = Some(receiver);
        self.sender = Some(sender);
    }

    fn request_cancel(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
            self.status = "İptal ediliyor…".into();
        }
    }

    fn finish_run(&mut self) {
        self.events = None;
        self.sender = None;
        self.cancel = None;
        self.stage = None;
        self.progress = None;
        self.stage_started = None;
    }

    fn drain_events(&mut self) {
        loop {
            let event = match self.events.as_ref().map(|rx| rx.try_recv()) {
                Some(Ok(event)) => event,
                Some(Err(TryRecvError::Empty)) => return,
                // The worker exited without a terminal event (panic); don't hang.
                Some(Err(TryRecvError::Disconnected)) | None => {
                    if self.is_running() {
                        self.finish_run();
                        if self.error.is_none() && self.summary.is_none() {
                            self.error = Some("işlem beklenmedik şekilde sona erdi".into());
                            self.status = "Hata".into();
                        }
                    }
                    return;
                }
            };

            match event {
                Event::Stage(stage) => {
                    self.stage = Some(stage);
                    self.status = format!("{}…", stage.label());
                    self.stage_started = Some(Instant::now());
                    self.stage_start_progress = 0.0;
                    self.progress = None;
                }
                Event::Status(text) => self.status = text,
                Event::Progress(value) => {
                    if self.progress.is_none()
                        && let Some(value) = value
                    {
                        // First reading of this stage: restart the clock here so
                        // model loading time doesn't skew the estimate.
                        self.stage_start_progress = value;
                        self.stage_started = Some(Instant::now());
                    }
                    self.progress = value;
                }
                Event::Segment(segment) => {
                    self.segments.push(segment);
                    self.text = transcribe::render(&self.segments, self.format);
                }
                Event::Finished {
                    segments,
                    audio_secs,
                    elapsed_secs,
                    transcribe_secs,
                    decoder,
                    suggested_stem,
                } => {
                    self.segments = segments;
                    self.text = transcribe::render(&self.segments, self.format);
                    self.suggested_stem = suggested_stem;
                    // Hız oranı yalnızca metne dönüştürmeden hesaplanır: ilk
                    // çalıştırmada araya giren model indirmesi oranı bozardı.
                    let speed = if transcribe_secs > 0.0 {
                        audio_secs / transcribe_secs
                    } else {
                        0.0
                    };
                    self.summary = Some(format!(
                        "{} parça · {:.0} sn ses · toplam {} · metne dönüştürme {} ({:.1}× gerçek zaman) · çözümleyici: {decoder}",
                        self.segments.len(),
                        audio_secs,
                        clock(Duration::from_secs_f64(elapsed_secs)),
                        clock(Duration::from_secs_f64(transcribe_secs)),
                        speed
                    ));
                    self.status = "Tamamlandı.".into();
                    self.finish_run();

                    if self.auto_save {
                        self.auto_save_output();
                    }
                }
                Event::Failed(message) => {
                    self.error = Some(message);
                    self.status = "Hata".into();
                    self.finish_run();
                }
                Event::Cancelled => {
                    self.status = "İptal edildi.".into();
                    self.finish_run();
                }
            }
        }
    }

    fn output_file_name(&self) -> String {
        format!("{}.{}", self.suggested_stem, self.format.extension())
    }

    fn auto_save_output(&mut self) {
        let Some(dir) = self.output_dir.clone() else {
            self.error = Some("kayıt klasörü belirlenemedi; \"Farklı kaydet\" kullanın".into());
            return;
        };
        let path = unique_path(&dir, &self.output_file_name());
        self.write_output(&path);
    }

    fn write_output(&mut self, path: &Path) {
        match std::fs::write(path, self.text.as_bytes()) {
            Ok(()) => {
                self.saved_to = Some(path.to_path_buf());
                self.status = format!("Kaydedildi: {}", path.display());
            }
            Err(err) => {
                self.error = Some(format!("dosya kaydedilemedi ({}): {err}", path.display()));
            }
        }
    }

    fn save_as(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_file_name(self.output_file_name())
            .add_filter(self.format.label(), &[self.format.extension()]);
        if let Some(dir) = &self.output_dir {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.save_file() {
            self.write_output(&path);
        }
    }

    fn pick_local_file(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Ses/video dosyaları",
                &["mp3", "wav", "m4a", "aac", "flac", "ogg", "opus", "mp4", "mkv", "mov", "webm"],
            )
            .add_filter("Tüm dosyalar", &["*"])
            .pick_file()
        {
            self.local_file = Some(path);
        }
    }
}

/// Avoids silently overwriting a previous transcript: `name.txt`, `name-2.txt`, …
fn unique_path(dir: &Path, file_name: &str) -> PathBuf {
    let candidate = dir.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = PathBuf::from(file_name);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "transkript".into());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "txt".into());

    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(file_name)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        if self.is_running() {
            // Keep polling the channel while work is in flight.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(8.0);
            ui.heading("Sesten Metne");
            ui.label(
                egui::RichText::new("whisper.cpp ile çevrimdışı konuşma tanıma")
                    .small()
                    .weak(),
            );
            ui.add_space(8.0);
        });

        egui::Panel::bottom("footer").show(ui, |ui| {
            ui.add_space(6.0);
            self.draw_footer(ui);
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            self.draw_source(ui);
            ui.add_space(6.0);
            self.draw_settings(ui);
            ui.add_space(6.0);
            self.draw_actions(ui);
            ui.add_space(4.0);
            self.draw_messages(ui);
            ui.separator();
            self.draw_output(ui);
        });
    }
}

impl App {
    fn draw_source(&mut self, ui: &mut egui::Ui) {
        let running = self.is_running();

        ui.horizontal(|ui| {
            ui.label("MP3 adresi:");
            let response = ui.add_enabled(
                !running && self.local_file.is_none(),
                egui::TextEdit::singleline(&mut self.url)
                    .hint_text("https://ornek.com/kayit.mp3")
                    .desired_width(f32::INFINITY),
            );
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !running {
                self.start();
            }
        });

        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new("Yerel dosya seç…"))
                .clicked()
            {
                self.pick_local_file();
            }
            if let Some(path) = self.local_file.clone() {
                ui.label(
                    egui::RichText::new(
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                    )
                    .strong(),
                );
                if ui
                    .add_enabled(!running, egui::Button::new("Kaldır"))
                    .on_hover_text("Yerel dosya seçimini bırak, adresi kullan")
                    .clicked()
                {
                    self.local_file = None;
                }
            } else {
                ui.label(
                    egui::RichText::new("veya bilgisayarınızdaki bir dosyayı kullanın")
                        .small()
                        .weak(),
                );
            }
        });
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        let running = self.is_running();

        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("settings")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    // The combo boxes sit in a grid with their captions in the
                    // left column, so each one is tied to its label explicitly —
                    // otherwise a screen reader announces an unnamed control.
                    let caption = ui.label("Model:");
                    let model = &MODELS[self.model_index];
                    egui::ComboBox::from_id_salt("model")
                        .width(420.0)
                        .selected_text(model_text(self.model_index, false))
                        .show_ui(ui, |ui| {
                            for index in 0..MODELS.len() {
                                ui.selectable_value(
                                    &mut self.model_index,
                                    index,
                                    model_text(index, true),
                                );
                            }
                        })
                        .response
                        .labelled_by(caption.id);
                    ui.end_row();

                    ui.label("");
                    ui.label(
                        egui::RichText::new(if model.is_downloaded() {
                            format!("Bu model indirildi ({})", model.size_label())
                        } else {
                            format!(
                                "Bu model ilk kullanımda indirilecek ({})",
                                model.size_label()
                            )
                        })
                        .small()
                        .weak(),
                    );
                    ui.end_row();

                    // English-only models can't be pointed at another language.
                    let multilingual = model.multilingual;
                    if !multilingual {
                        self.language_index = transcribe::english_index();
                    }

                    let caption = ui.label("Konuşma dili:");
                    ui.add_enabled_ui(multilingual, |ui| {
                        egui::ComboBox::from_id_salt("language")
                            .width(220.0)
                            .selected_text(transcribe::LANGUAGES[self.language_index].0)
                            .show_ui(ui, |ui| {
                                for (index, (label, _)) in transcribe::LANGUAGES.iter().enumerate()
                                {
                                    ui.selectable_value(&mut self.language_index, index, *label);
                                }
                            })
                            .response
                            .labelled_by(caption.id)
                            .on_disabled_hover_text("Bu model yalnızca İngilizce destekler");
                    });
                    ui.end_row();

                    let caption = ui.label("Çıktı biçimi:");
                    let previous = self.format;
                    egui::ComboBox::from_id_salt("format")
                        .width(220.0)
                        .selected_text(self.format.label())
                        .show_ui(ui, |ui| {
                            for format in OutputFormat::ALL {
                                ui.selectable_value(&mut self.format, format, format.label());
                            }
                        })
                        .response
                        .labelled_by(caption.id);
                    if self.format != previous && !self.segments.is_empty() {
                        self.text = transcribe::render(&self.segments, self.format);
                        self.saved_to = None;
                    }
                    ui.end_row();
                });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.translate, "İngilizceye çevir")
                    .on_hover_text("Konuşmayı olduğu gibi yazmak yerine İngilizceye çevirir");
                ui.separator();
                ui.checkbox(&mut self.auto_save, "Bitince otomatik kaydet");
                if self.auto_save {
                    let label = self
                        .output_dir
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "(klasör seçilmedi)".into());
                    if ui
                        .button("Klasör…")
                        .on_hover_text(label.clone())
                        .clicked()
                        && let Some(dir) = rfd::FileDialog::new().pick_folder()
                    {
                        self.output_dir = Some(dir);
                    }
                    ui.label(egui::RichText::new(short_path(&label)).small().weak());
                }
            });

            ui.collapsing("Gelişmiş", |ui| {
                ui.add(egui::Slider::new(&mut self.threads, 1..=32).text("İş parçacığı"));
                // GPU hızlandırma bir tercih değil: varsa her zaman kullanılır.
                // Burada yalnızca durumu bildiriyoruz.
                let (text, hint) = match transcribe::GPU_BACKEND {
                    Some(backend) => (
                        format!("Hızlandırma: {backend}"),
                        "GPU kullanılamazsa işlem kendiliğinden CPU'ya düşer",
                    ),
                    None => (
                        "Hızlandırma: CPU".to_string(),
                        "Bu platformda GPU arka ucu yok; işlem CPU'da çalışır",
                    ),
                };
                ui.label(egui::RichText::new(text).small().weak())
                    .on_hover_text(hint);
                ui.label("Sözlük ipucu (özel isimler, terimler):");
                ui.add(
                    egui::TextEdit::multiline(&mut self.prompt)
                        .desired_rows(2)
                        .desired_width(f32::INFINITY)
                        .hint_text("Örn: Ankara, Boğaziçi, mutabakat"),
                );
            });
        });
    }

    fn draw_actions(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.is_running() {
                if ui.button("İptal").clicked() {
                    self.request_cancel();
                }
            } else {
                let ready = self.local_file.is_some() || !self.url.trim().is_empty();
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(egui::RichText::new("Metne Dönüştür").strong())
                            .min_size(egui::vec2(150.0, 28.0)),
                    )
                    .clicked()
                {
                    self.start();
                }
            }

            if self.is_running() {
                let bar = match self.progress {
                    Some(value) => egui::ProgressBar::new(value).show_percentage(),
                    None => egui::ProgressBar::new(0.0).animate(true).text("çalışıyor"),
                };
                ui.add(bar.desired_width(220.0));

                // Elapsed time always moves, even when the percentage sits on
                // the same value for minutes — that is the difference between
                // "slow" and "frozen", and the user can't tell without it.
                if let Some(timing) = self.timing() {
                    ui.label(egui::RichText::new(timing).small().weak());
                }
            }

            ui.label(&self.status);
        });
    }

    /// `geçen 04:12 · kalan ~18 dk` — the estimate appears only once there is
    /// enough progress for it to mean anything.
    fn timing(&self) -> Option<String> {
        let started = self.stage_started?;
        let elapsed = started.elapsed();
        let mut text = format!("geçen {}", clock(elapsed));

        if let Some(progress) = self.progress {
            let covered = progress - self.stage_start_progress;
            let remaining = 1.0 - progress;
            if covered > 0.02 && remaining > 0.0 && elapsed.as_secs_f32() > 5.0 {
                let per_unit = elapsed.as_secs_f32() / covered;
                text.push_str(&format!(" · kalan ~{}", rough(remaining * per_unit)));
            }
        }

        Some(text)
    }

    fn draw_messages(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.error.clone() {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "Hata:");
                ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
            });
        }
        if let Some(summary) = &self.summary {
            ui.label(egui::RichText::new(summary).small().weak());
        }
    }

    fn draw_output(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Çıktı").strong());
            if !self.text.is_empty() {
                ui.label(
                    egui::RichText::new(format!("{} karakter", self.text.chars().count()))
                        .small()
                        .weak(),
                );
            }
        });

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(self.is_running())
            .show(ui, |ui| {
                ui.add_sized(
                    ui.available_size(),
                    egui::TextEdit::multiline(&mut self.text)
                        .hint_text("Metin burada görünecek.")
                        .desired_width(f32::INFINITY),
                );
            });
    }

    fn draw_footer(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let has_text = !self.text.trim().is_empty();
            if ui
                .add_enabled(
                    has_text,
                    egui::Button::new(format!(
                        "{} olarak kaydet",
                        self.format.extension().to_uppercase()
                    )),
                )
                .clicked()
            {
                self.save_as();
            }
            if ui
                .add_enabled(has_text, egui::Button::new("Kopyala"))
                .clicked()
            {
                ui.ctx().copy_text(self.text.clone());
            }
            if let Some(path) = self.saved_to.clone()
                && ui
                    .button("Klasörü aç")
                    .on_hover_text(path.display().to_string())
                    .clicked()
            {
                reveal(&path);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The full path is too long for the footer; keep it in the tooltip.
                ui.label(egui::RichText::new("önbellek klasörü").small().weak())
                    .on_hover_text(format!(
                        "İndirilen modeller ve ffmpeg burada tutulur:\n{}",
                        paths::cache_root_for_display()
                    ));
            });
        });
    }
}

/// `in_list` adds the download marker, which only helps while browsing the
/// dropdown — the closed box already has the state spelled out underneath it.
/// `mm:ss`, or `h:mm:ss` once it runs past an hour.
fn clock(duration: Duration) -> String {
    let total = duration.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// A deliberately coarse estimate: the precision of "kalan ~2 sa 10 dk" would
/// be a lie, and a number that jitters every second reads as broken.
fn rough(seconds: f32) -> String {
    let seconds = seconds.max(0.0) as u64;
    match seconds {
        0..=90 => format!("{seconds} sn"),
        91..=3599 => format!("{} dk", (seconds + 30) / 60),
        _ => {
            let hours = seconds / 3600;
            let minutes = (seconds % 3600) / 60;
            if minutes >= 5 {
                format!("{hours} sa {} dk", (minutes / 5) * 5)
            } else {
                format!("{hours} sa")
            }
        }
    }
}

fn model_text(index: usize, in_list: bool) -> String {
    let model = &MODELS[index];
    let mark = match (in_list, model.is_downloaded()) {
        (true, true) => "• ",
        (true, false) => "   ",
        (false, _) => "",
    };
    format!("{mark}{} · {}", model.label, model.size_label())
}

fn short_path(path: &str) -> String {
    const MAX: usize = 46;
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= MAX {
        return path.to_string();
    }
    format!("…{}", chars[chars.len() - MAX + 1..].iter().collect::<String>())
}

/// Opens the system file browser at the saved transcript.
fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();

    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::{clock, rough, short_path};
    use std::time::Duration;

    #[test]
    fn formats_elapsed_clock() {
        assert_eq!(clock(Duration::from_secs(0)), "00:00");
        assert_eq!(clock(Duration::from_secs(62)), "01:02");
        assert_eq!(clock(Duration::from_secs(3661)), "1:01:01");
    }

    #[test]
    fn estimates_stay_coarse() {
        assert_eq!(rough(45.0), "45 sn");
        assert_eq!(rough(90.0), "90 sn");
        // Just past the seconds cutoff it rounds to the nearest minute.
        assert_eq!(rough(100.0), "2 dk");
        assert_eq!(rough(1800.0), "30 dk");
        // Hours are rounded down to five-minute steps, never to false precision.
        assert_eq!(rough(7_000.0), "1 sa 55 dk");
        assert_eq!(rough(3_600.0), "1 sa");
        assert_eq!(rough(-5.0), "0 sn");
    }

    #[test]
    fn shortens_long_paths_from_the_left() {
        assert_eq!(short_path("/tmp/a"), "/tmp/a");
        let long = "/Users/someone/very/deeply/nested/directory/structure/downloads";
        let short = super::short_path(long);
        assert!(short.starts_with('…'));
        assert!(long.ends_with(short.trim_start_matches('…')));
    }
}
