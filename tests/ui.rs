//! Headless rendering of the main window.
//!
//! This catches layout mistakes that a compile check can't: a panel that eats
//! the whole window, a control that never gets drawn, a label clipped away, or
//! a character the bundled font has no glyph for.
//!
//! The snapshots must be reproducible on any machine, so everything the window
//! reads from its environment is pinned first: the cache directory (which
//! decides whether a model shows as downloaded), the output folder, and the
//! thread count (which follows the CPU's core count). Without that, the images
//! only ever match on the machine that produced them.
//!
//! Run with `UPDATE_SNAPSHOTS=1 cargo test --test ui` to refresh the images in
//! `tests/snapshots/` after an intentional UI change.

use std::sync::Once;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// Points the app at an empty cache, so no model reads as already downloaded.
fn pin_environment() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join("sesten-metne-ui-test-cache");
        std::fs::create_dir_all(&dir).expect("geçici önbellek dizini");
        // SAFETY: runs once, before any harness exists, and every caller sets
        // the same value, so no thread can observe a half-written change.
        unsafe { std::env::set_var(whisper_gui::paths::CACHE_DIR_ENV, &dir) };
    });
}

fn harness<'a>() -> Harness<'a, whisper_gui::App> {
    pin_environment();
    Harness::builder()
        .with_size(egui::vec2(920.0, 720.0))
        .wgpu()
        .build_eframe(|cc| whisper_gui::App::for_snapshot_test(cc))
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
