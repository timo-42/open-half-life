//! Read-only graphics diagnostics drawn over gameplay.

use egui::{Align2, Color32, Frame, Grid, RichText, Stroke};

/// Values shown by the graphics debug overlay.
#[derive(Debug, Clone, Copy)]
pub struct GraphicsDebugInfo<'a> {
    /// Average frames per second over the most recent second.
    pub fps: f64,
    /// Average FPS of the slowest one percent of frames retained for 60 seconds.
    pub one_percent_low_fps: f64,
    /// Duration of the most recently completed frame.
    pub frame_ms: f64,
    /// CPU time spent advancing simulation in the most recent frame.
    pub simulation_ms: f64,
    /// CPU time spent acquiring the window surface in the most recent frame.
    pub acquire_ms: f64,
    /// CPU time spent preparing and submitting game rendering.
    pub render_ms: f64,
    /// CPU time spent on UI submission and presentation.
    pub ui_present_ms: f64,
    /// Render-target width in physical pixels.
    pub width: u32,
    /// Render-target height in physical pixels.
    pub height: u32,
    /// Graphics adapter name reported by wgpu.
    pub adapter: &'a str,
    /// Graphics API backend reported by wgpu.
    pub backend: &'a str,
    /// Cumulative bytes uploaded for static brush resources in this level.
    pub static_upload_bytes: u64,
    /// Cumulative texture uploads for brush resources in this level.
    pub texture_uploads: u64,
    /// Cumulative dynamic lightmap updates in this level.
    pub lightmap_uploads: u64,
}

/// Draws a compact, non-interactive panel in the top-right corner.
pub fn draw(ctx: &egui::Context, info: &GraphicsDebugInfo<'_>) {
    egui::Window::new("Graphics debug")
        .anchor(Align2::RIGHT_TOP, [-12.0, 12.0])
        .collapsible(false)
        .resizable(false)
        .title_bar(true)
        .interactable(false)
        .frame(
            Frame::new()
                .inner_margin(8)
                .fill(Color32::from_black_alpha(220))
                .stroke(Stroke::new(1.0, Color32::from_gray(90))),
        )
        .show(ctx, |ui| {
            Grid::new("ohl_graphics_debug_grid")
                .num_columns(2)
                .spacing([18.0, 3.0])
                .show(ui, |ui| {
                    row(ui, "FPS", format!("{:.1}", info.fps));
                    row(
                        ui,
                        "1% low (60 s)",
                        format!("{:.1} FPS", info.one_percent_low_fps),
                    );
                    row(ui, "Frame time", format!("{:.2} ms", info.frame_ms));
                    row(
                        ui,
                        "Resolution",
                        format!("{} x {}", info.width, info.height),
                    );
                    row(ui, "GPU", info.adapter);
                    row(ui, "Backend", info.backend);
                    ui.separator();
                    ui.separator();
                    row(
                        ui,
                        "Simulation CPU",
                        format!("{:.2} ms", info.simulation_ms),
                    );
                    row(ui, "Render submit CPU", format!("{:.2} ms", info.render_ms));
                    row(ui, "Surface acquire", format!("{:.2} ms", info.acquire_ms));
                    row(ui, "UI + present", format!("{:.2} ms", info.ui_present_ms));
                    row(
                        ui,
                        "Brush resource uploads",
                        format_mib(info.static_upload_bytes),
                    );
                    row(ui, "Texture uploads", info.texture_uploads.to_string());
                    row(ui, "Lightmap updates", info.lightmap_uploads.to_string());
                });
            ui.add_space(3.0);
            ui.label(RichText::new("P: close").small().weak());
        });
}

fn row(ui: &mut egui::Ui, label: &str, value: impl Into<egui::WidgetText>) {
    ui.label(RichText::new(label).weak());
    ui.label(value);
    ui.end_row();
}

fn format_mib(bytes: u64) -> String {
    const MIB: u64 = 1_048_576;
    format!("{}.{:01} MiB", bytes / MIB, (bytes % MIB) * 10 / MIB)
}
