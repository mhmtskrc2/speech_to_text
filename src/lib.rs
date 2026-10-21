//! Offline speech-to-text: fetch audio from a URL, decode it, and run it
//! through whisper.cpp — downloading the model (and ffmpeg, if needed) the
//! first time each is required.
//!
//! The binary wraps this in an egui window; everything here is UI-independent
//! apart from [`app`], which is exposed so the window can be rendered headlessly
//! in tests.

pub mod app;
pub mod audio;
pub mod cli;
pub mod ffmpeg;
pub mod job;
pub mod models;
pub mod net;
pub mod paths;
pub mod transcribe;

pub use app::App;
