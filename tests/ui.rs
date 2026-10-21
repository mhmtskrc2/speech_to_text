//! Headless rendering of the main window.
//!
//! This catches layout mistakes that a compile check can't: a panel that eats
//! the whole window, a control that never gets drawn, a label clipped away.
//! Run with `UPDATE_SNAPSHOTS=1 cargo test --test ui` to refresh the images in
//! `tests/snapshots/` after an intentional UI change.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn harness<'a>() -> Harness<'a, whisper_gui::App> {
    Harness::builder()
        .with_size(egui::vec2(920.0, 720.0))
        .wgpu()
        .build_eframe(|cc| whisper_gui::App::new(cc))
}

#[test]
fn main_window_renders() {
    let mut harness = harness();
    harness.run();

    // The controls a first-time user needs must all be on screen.
    for label in [
        "Sesten Metne",
        "MP3 adresi:",
        "Model:",
        "Konuşma dili:",
        "Çıktı biçimi:",
        "Metne Dönüştür",
        "Yerel dosya seç…",
        "Bitince otomatik kaydet",
    ] {
        assert!(
            harness.query_by_label(label).is_some(),
            "arayüzde bulunamadı: {label}"
        );
    }

    harness.snapshot("main_window");
}

#[test]
fn advanced_section_expands() {
    let mut harness = harness();
    harness.run();

    harness.get_by_label("Gelişmiş").click();
    harness.run();

    // The slider reports itself as both a Slider and a SpinButton node.
    assert!(
        harness.query_all_by_label("İş parçacığı").next().is_some(),
        "gelişmiş bölüm açılmadı"
    );

    harness.snapshot("advanced_open");
}
