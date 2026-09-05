//! A helper to build the one full-screen root [`egui::Ui`] that top-level
//! panels (the console's bar, the menu's central panel) attach to during a
//! frame, mirroring the construction egui's own `Context::run_ui` uses
//! internally.

/// Creates a full-viewport root `Ui` for this pass. Callers show top-level
/// panels against it (in the order they should stack) between
/// [`crate::UiLayer::begin_frame`]/[`crate::UiLayer::begin_frame_headless`]
/// and [`crate::UiLayer::end_frame_and_render`].
#[must_use]
pub fn root_ui(ctx: &egui::Context) -> egui::Ui {
    egui::Ui::new(
        ctx.clone(),
        egui::Id::new((ctx.viewport_id(), "ohl_ui_root")),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    )
}
