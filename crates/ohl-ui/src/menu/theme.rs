//! The menu's look and the page chrome every pane shares: the panel, the
//! breadcrumb and title, a scrolling content area and a footer with BACK.

use egui::{Color32, RichText, Stroke, Vec2};

use super::MenuState;

/// The highlight colour: the logo, selections, the active widget.
pub(super) const ACCENT: Color32 = Color32::from_rgb(246, 154, 38);
/// Titles and headings.
pub(super) const TITLE: Color32 = Color32::from_rgb(235, 225, 195);
/// Body text.
pub(super) const TEXT: Color32 = Color32::from_rgb(235, 225, 195);
/// Secondary text: hints, descriptions, unavailable entries.
pub(super) const TEXT_DIM: Color32 = Color32::from_rgb(150, 146, 122);
/// Panel and widget outlines.
pub(super) const BORDER: Color32 = Color32::from_rgb(104, 111, 82);
/// The main menu's opaque background.
pub(super) const BACKDROP: Color32 = Color32::from_rgb(12, 18, 14);
/// A warning line's colour.
pub(super) const WARNING: Color32 = Color32::from_rgb(232, 186, 92);

/// Width of a root-pane or hub button.
pub(super) const NAV_WIDTH: f32 = 300.0;
/// Height reserved below a page's content for its footer.
const FOOTER_HEIGHT: f32 = 52.0;
/// The largest a page panel grows.
const PAGE_MAX: Vec2 = Vec2::new(860.0, 620.0);

/// The pause menu's backdrop: the paused game stays visible through it.
pub(super) fn pause_backdrop() -> Color32 {
    Color32::from_rgba_unmultiplied(6, 10, 8, 215)
}

/// `style` with the menu's colours and spacing.
pub(super) fn styled(mut style: egui::Style) -> egui::Style {
    // The menu is always dark, whatever the system's theme.
    style.visuals = egui::Visuals::dark();
    let visuals = &mut style.visuals;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = Color32::from_rgb(145, 76, 20);
    visuals.selection.stroke = Stroke::new(1.0, TITLE);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(35, 42, 35);
    visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(35, 42, 35);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(124, 68, 22);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(124, 68, 22);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.active.bg_fill = visuals.selection.bg_fill;
    visuals.widgets.active.weak_bg_fill = visuals.selection.bg_fill;
    visuals.extreme_bg_color = Color32::from_rgb(8, 12, 9);
    visuals.window_fill = Color32::from_rgb(20, 27, 22);
    visuals.panel_fill = Color32::from_rgb(20, 27, 22);
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(10.0, 8.0);
    spacing.button_padding = Vec2::new(12.0, 5.0);
    spacing.slider_width = 260.0;
    spacing.interact_size.y = 26.0;
    style
}

/// Draws `contents` in the menu's style: `ui` and everything inside it,
/// and the popups (a combo box's list) it opens, which take their style
/// from the context rather than from `ui`. The context's own style is put
/// back afterwards, so the console and HUD keep theirs.
pub(super) fn with_style<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    let previous = ctx.global_style();
    let style = styled((*previous).clone());
    ctx.set_global_style(style.clone());
    *ui.style_mut() = style;
    let result = contents(ui);
    ctx.set_global_style(previous);
    result
}

/// A root-pane or hub button, its label to the left.
pub(super) fn nav_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new((
            RichText::new(label.to_uppercase()).size(19.0).strong(),
            egui::Atom::grow(),
        ))
        .min_size(Vec2::new(NAV_WIDTH, 38.0)),
    )
}

/// One row of a selectable list, as wide as the list, its text to the
/// left.
pub(super) fn list_row(
    ui: &mut egui::Ui,
    selected: bool,
    text: impl Into<egui::WidgetText>,
    height: f32,
) -> egui::Response {
    let width = ui.available_width();
    ui.add(
        egui::Button::selectable(selected, (text.into(), egui::Atom::grow()))
            .min_size(Vec2::new(width, height)),
    )
}

/// A hub entry: a [`nav_button`] with a one-line description beside it.
pub(super) fn hub_entry(ui: &mut egui::Ui, label: &str, description: &str) -> egui::Response {
    ui.horizontal(|ui| {
        let response = nav_button(ui, label);
        ui.label(RichText::new(description).color(TEXT_DIM));
        response
    })
    .inner
}

/// A footer or form button.
pub(super) fn button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label.to_uppercase()).size(16.0).strong())
            .min_size(Vec2::new(120.0, 32.0)),
    )
}

/// A section heading inside a page.
pub(super) fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(17.0).strong().color(ACCENT));
}

/// A muted explanatory line.
pub(super) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(TEXT_DIM));
}

/// A boxed notice that a pane is a preview of something not implemented.
pub(super) fn preview_banner(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgba_unmultiplied(70, 50, 14, 160))
        .stroke(Stroke::new(1.0, WARNING))
        .corner_radius(3.0)
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(WARNING));
        });
}

/// Draws a page: a centred panel with the breadcrumb and the current pane's
/// title, then whatever `contents` adds (normally a [`content`] area and a
/// [`footer`]).
pub(super) fn page(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    contents: impl FnOnce(&mut egui::Ui, &mut MenuState),
) {
    let full = ui.max_rect();
    let size = Vec2::new(
        (full.width() - 48.0).clamp(320.0, PAGE_MAX.x),
        (full.height() - 48.0).clamp(240.0, PAGE_MAX.y),
    );
    let rect = egui::Rect::from_center_size(full.center(), size);
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        egui::Frame::new()
            .fill(Color32::from_rgba_unmultiplied(20, 27, 22, 240))
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                let trail = state.trail();
                let crumbs: Vec<&str> = std::iter::once("Open Half-Life")
                    .chain(trail.iter().map(|pane| pane.title()))
                    .collect();
                if let Some((title, path)) = crumbs.split_last() {
                    ui.label(
                        RichText::new(path.join("  ›  ").to_uppercase())
                            .size(12.0)
                            .color(TEXT_DIM),
                    );
                    ui.label(
                        RichText::new(title.to_uppercase())
                            .size(26.0)
                            .strong()
                            .color(TITLE),
                    );
                }
                ui.add_space(2.0);
                ui.separator();
                contents(ui, state);
            });
    });
}

/// A page's scrolling content area, leaving room below for its footer.
pub(super) fn content<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let height = (ui.available_height() - FOOTER_HEIGHT).max(60.0);
    egui::ScrollArea::vertical()
        .id_salt("ohl_menu_page")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .show(ui, contents)
        .inner
}

/// A page's footer: BACK on the left, `right` (laid out right to left) on
/// the right.
pub(super) fn footer(
    ui: &mut egui::Ui,
    state: &mut MenuState,
    right: impl FnOnce(&mut egui::Ui, &mut MenuState),
) {
    ui.separator();
    ui.horizontal(|ui| {
        if button(ui, "Back").clicked() {
            state.back();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right(ui, state);
        });
    });
}
