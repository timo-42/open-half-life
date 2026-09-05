//! egui presentation for [`Console`].

use egui::{Color32, Key, ScrollArea, TextEdit};

use super::Console;

/// Draws the console as a panel dropped from the top of the screen when
/// open. Returns `true` if the console consumed keyboard focus this frame
/// (the caller uses this to decide whether gameplay input should also see
/// the keys). `ui` is the frame's root `Ui`; see [`crate::root_ui`].
pub fn draw(ui: &mut egui::Ui, console: &mut Console) -> bool {
    if !console.is_open() {
        return false;
    }

    let mut submit = false;
    let mut want_tab = false;
    let mut want_up = false;
    let mut want_down = false;

    egui::Panel::top("ohl_console")
        .min_size(220.0)
        .show(ui, |ui| {
            ui.set_min_height(200.0);
            ScrollArea::vertical()
                .max_height(160.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in console.buffer().lines() {
                        ui.colored_label(Color32::from_gray(220), line);
                    }
                });

            ui.horizontal(|ui| {
                ui.label(">");
                let response = ui.add(
                    TextEdit::singleline(console.input_mut())
                        .desired_width(f32::INFINITY)
                        .hint_text("command"),
                );
                response.request_focus();
                if response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
                    submit = true;
                }
                if response.has_focus() {
                    ui.input(|input| {
                        want_tab = input.key_pressed(Key::Tab);
                        want_up = input.key_pressed(Key::ArrowUp);
                        want_down = input.key_pressed(Key::ArrowDown);
                    });
                }
            });
        });

    if want_tab {
        console.apply_tab_completion();
    }
    if want_up {
        console.history_previous();
    }
    if want_down {
        console.history_next();
    }
    if submit {
        console.submit_input();
    }

    true
}

#[cfg(test)]
mod tests {
    use super::super::Console;
    use super::draw;
    use crate::root_ui;

    #[test]
    fn closed_console_does_not_capture_input() {
        let ctx = egui::Context::default();
        let mut console = Console::new();
        ctx.begin_pass(egui::RawInput::default());
        let mut ui = root_ui(&ctx);
        let captured = draw(&mut ui, &mut console);
        let mut output = ctx.end_pass();
        // A real renderer would apply these; the test only exercises input
        // capture, so just acknowledge them to satisfy `FullOutput`'s
        // must-be-handled invariant on drop.
        output.textures_delta.clear();
        assert!(!captured);
    }

    #[test]
    fn open_console_captures_input() {
        let ctx = egui::Context::default();
        let mut console = Console::new();
        console.set_open(true);
        ctx.begin_pass(egui::RawInput::default());
        let mut ui = root_ui(&ctx);
        let captured = draw(&mut ui, &mut console);
        let mut output = ctx.end_pass();
        // A real renderer would apply these; the test only exercises input
        // capture, so just acknowledge them to satisfy `FullOutput`'s
        // must-be-handled invariant on drop.
        output.textures_delta.clear();
        assert!(captured);
    }
}
