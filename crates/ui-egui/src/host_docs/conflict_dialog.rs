//! The "keep mine / use theirs" window of `regionConflict`.

/// Widest the question's text runs, as on a desktop.
const TEXT_W: f32 = 360.0;
/// Room kept between the window and each edge of a narrow screen.
const EDGE: f32 = 16.0;

/// The text width that keeps the window (`chrome`: its margins and border) `EDGE` inside a
/// `screen` wide screen, never narrower than a short word.
fn text_width(screen: f32, chrome: f32) -> f32 {
    (screen - 2.0 * EDGE - chrome).clamp(120.0, TEXT_W)
}

/// Shows the question for `region` and returns the answer once a button is clicked.
pub(super) fn show(ctx: &egui::Context, region: &str) -> Option<&'static str> {
    let mut answer = None;
    let style = ctx.global_style();
    let chrome = style.spacing.window_margin.sum().x + 2.0 * style.visuals.window_stroke.width;
    let width = text_width(ctx.content_rect().width(), chrome);
    let title = tl!("Changed while you were away");
    // egui's title bar doesn't wrap and keeps room for buttons on both sides, so the title is
    // drawn in the window instead: it wraps like the text on a narrow screen.
    egui::Window::new(title)
        .id(egui::Id::new("live_conflict").with(region))
        .title_bar(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
        .show(ctx, |ui| {
            ui.set_max_width(width);
            ui.add(egui::Label::new(egui::RichText::new(title).heading()).wrap());
            ui.separator();
            ui.label(tl!("Someone else changed this part while you were offline. Which version do you keep?"));
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                if ui.button(tl!("Keep mine")).clicked() {
                    answer = Some("mine");
                }
                if ui.button(tl!("Use theirs")).clicked() {
                    answer = Some("theirs");
                }
            });
        });
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window's rect after a few frames of a `width` wide screen.
    fn window_at(width: f32) -> egui::Rect {
        let ctx = egui::Context::default();
        for i in 0..4 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 780.0))),
                time: Some(f64::from(i) * 0.1),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                let _ = show(ui.ctx(), "para:00000001");
            })
            .drop_without_applying_deltas();
        }
        let id = egui::Id::new("live_conflict").with("para:00000001");
        ctx.memory(|m| m.area_rect(id)).expect("the conflict window is shown")
    }

    #[test]
    fn the_text_narrows_only_below_a_desktop_width() {
        assert_eq!(text_width(1280.0, 14.0), TEXT_W);
        assert_eq!(text_width(360.0, 14.0), 360.0 - 32.0 - 14.0);
        assert_eq!(text_width(100.0, 14.0), 120.0);
    }

    #[test]
    fn narrow_screens_keep_margins_around_the_conflict_window() {
        for width in [360.0, 280.0] {
            let r = window_at(width);
            assert!(r.left() >= 14.0 && r.right() <= width - 14.0, "the window {:?} reaches the edges of a {width} screen", [r.left(), r.right()]);
        }
        let wide = window_at(1280.0);
        assert!(wide.width() <= 380.0 && wide.width() > window_at(360.0).width(), "a wide screen keeps the 360 text width: {}", wide.width());
    }
}
