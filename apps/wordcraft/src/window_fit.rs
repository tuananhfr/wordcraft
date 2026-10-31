//! With no saved window geometry, the window opens maximized, and Restore gives a window that fits
//! the screen.
//!
//! eframe only clamps the start size to the largest monitor's full size, ignoring the taskbar and
//! the title bar, so on a 1366×768 screen or at 150 % scaling the default window ran off the screen.
//! egui knows the monitor only once the window exists, so the restore size is set on the first
//! frame (before eframe shows the window), then the window is maximized.

use egui::{Pos2, Vec2, ViewportCommand, vec2};
use wordcraft_ui_egui::window_geometry::{MIN_SIZE, WindowGeometry};

pub const DEFAULT_INNER_SIZE: Vec2 = vec2(1440.0, 920.0);
/// Kept free at the bottom of the screen: Windows 11's taskbar height. egui can't see the real one.
const TASKBAR: f32 = 48.0;
/// Kept free between the restored window and the screen edges.
const GAP: f32 = 16.0;
/// Title bar and borders, for when the window reports no outer rect.
const FALLBACK_FRAME: Vec2 = vec2(16.0, 40.0);

/// Outer position and inner size of the restored window on a `monitor` whose origin is (0, 0).
/// `frame` is the outer size minus the inner size.
pub fn restored_rect(monitor: Vec2, frame: Vec2) -> (Pos2, Vec2) {
    let usable = vec2(monitor.x, monitor.y - TASKBAR);
    let inner = (usable - frame - Vec2::splat(2.0 * GAP)).min(DEFAULT_INNER_SIZE).max(MIN_SIZE);
    let outer = inner + frame;
    (((usable - outer) / 2.0).max(Vec2::ZERO).to_pos2(), inner)
}

pub struct WindowFit {
    pending: bool,
}

impl WindowFit {
    /// Fits the window only when there is no saved geometry to restore.
    pub fn new(restored: Option<WindowGeometry>) -> Self {
        Self { pending: restored.is_none() }
    }

    /// Sets the restore rect and maximizes, once, and returns the geometry to save: tracking would
    /// otherwise keep the unfitted size the window had before it was maximized. Positions assume the
    /// primary monitor, where `NativeOptions::centered` puts the window.
    pub fn apply(&mut self, ctx: &egui::Context) -> Option<WindowGeometry> {
        if !self.pending {
            return None;
        }
        let (monitor, frame) = ctx.input(|input| {
            let viewport = input.viewport();
            let frame = match (viewport.outer_rect, viewport.inner_rect) {
                (Some(outer), Some(inner)) => outer.size() - inner.size(),
                _ => FALLBACK_FRAME,
            };
            viewport.monitor_size.map(|monitor| (monitor, frame))
        })?;
        self.pending = false;
        let (position, size) = restored_rect(monitor, frame);
        // InnerSize un-maximizes on Windows (winit), so it must come before Maximized.
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(size));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(position));
        ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
        Some(WindowGeometry {
            position: Some([position.x, position.y]),
            width: size.x,
            height: size.y,
            maximized: true,
            monitor: Some([monitor.x, monitor.y]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    const FRAME: Vec2 = vec2(16.0, 39.0);

    fn fits(monitor: Vec2) {
        let (position, inner) = restored_rect(monitor, FRAME);
        let outer_max = position + inner + FRAME;
        assert!(position.x >= 0.0 && position.y >= 0.0, "{monitor:?}: {position:?}");
        assert!(outer_max.x <= monitor.x && outer_max.y <= monitor.y - TASKBAR, "{monitor:?}: ends at {outer_max:?}");
        assert!(inner.x <= DEFAULT_INNER_SIZE.x && inner.y <= DEFAULT_INNER_SIZE.y);
    }

    #[test]
    fn restored_window_fits_small_and_scaled_screens() {
        // 1920×1080 at 150 %, 1366×768 at 100 %, 1920×1080 at 125 %.
        for monitor in [vec2(1280.0, 720.0), vec2(1366.0, 768.0), vec2(1536.0, 864.0)] {
            fits(monitor);
        }
        assert_eq!(restored_rect(vec2(1280.0, 720.0), FRAME), (pos2(16.0, 16.0), vec2(1232.0, 601.0)));
    }

    #[test]
    fn large_screen_keeps_the_default_size_centred() {
        fits(vec2(1920.0, 1080.0));
        let (position, inner) = restored_rect(vec2(1920.0, 1080.0), FRAME);
        assert_eq!(inner, DEFAULT_INNER_SIZE);
        assert_eq!(position, pos2((1920.0 - 1456.0) / 2.0, (1032.0 - 959.0) / 2.0));
    }

    #[test]
    fn tiny_screen_never_goes_below_the_minimum_size() {
        let (position, inner) = restored_rect(vec2(600.0, 400.0), FRAME);
        assert_eq!(inner, MIN_SIZE);
        assert_eq!(position, Pos2::ZERO);
    }

    #[test]
    fn saved_geometry_is_restored_not_fitted() {
        let saved = WindowGeometry { position: Some([10.0, 10.0]), width: 900.0, height: 600.0, maximized: false, monitor: None };
        assert!(!WindowFit::new(Some(saved)).pending);
        assert!(WindowFit::new(None).pending);
    }
}
