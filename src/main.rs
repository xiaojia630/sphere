//! Sphere OS: a desktop where agents do the work and you steer.
//!
//! The same code runs as a native app (`cargo run --release`) and in the
//! browser (`./build-web.sh`).

// egui takes stroke widths as `impl Into<f32>`, so plain float literals rely
// on f32 fallback, which newer compilers warn about.
#![allow(unknown_lints, float_literal_f32_fallback)]

mod app;
mod apps;
mod core_view;
mod icons;
mod runtime;
mod services;
mod theme;

use app::SphereApp;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("Sphere OS"),
        ..Default::default()
    };
    eframe::run_native(
        "Sphere OS",
        options,
        Box::new(|cc| Ok(Box::new(SphereApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast;

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .expect("no window")
            .document()
            .expect("no document");
        let canvas = document
            .get_element_by_id("sphere")
            .expect("no #sphere canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#sphere is not a canvas");
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(SphereApp::new(cc)))),
            )
            .await;
        if let Some(loading) = document.get_element_by_id("loading") {
            match started {
                Ok(()) => loading.remove(),
                Err(e) => loading.set_text_content(Some(&format!("Could not start: {e:?}"))),
            }
        }
    });
}
