//! Platform-independent geometry for the GUI overlay scrollbar.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    pub fn height(self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VerticalScrollbar {
    pub track: Rect,
    pub thumb: Rect,
    pub max_top: usize,
}

impl VerticalScrollbar {
    pub fn new(
        track: Rect,
        total: usize,
        visible: usize,
        top: usize,
        min_thumb: f32,
    ) -> Option<Self> {
        let visible = visible.max(1);
        let max_top = total.saturating_sub(visible);
        if max_top == 0 || track.height() <= 0.0 || track.right <= track.left {
            return None;
        }
        let height = track.height();
        let thumb_height = (height * visible as f32 / total as f32)
            .max(min_thumb.max(0.0).min(height * 0.8))
            .min(height);
        let thumb_top =
            track.top + (height - thumb_height) * (top.min(max_top) as f64 / max_top as f64) as f32;
        Some(Self {
            track,
            thumb: Rect {
                top: thumb_top,
                bottom: thumb_top + thumb_height,
                ..track
            },
            max_top,
        })
    }

    /// Map a pointer position to a top line, retaining the original grab point
    /// within the thumb. Capture allows dragging beyond the client rectangle.
    pub fn top_for_pointer(self, y: f32, grab_offset: f32) -> usize {
        let travel = self.track.height() - self.thumb.height();
        if travel <= 0.0 {
            return 0;
        }
        let fraction = ((y - self.track.top - grab_offset) / travel).clamp(0.0, 1.0);
        (fraction as f64 * self.max_top as f64).round() as usize
    }
}

pub fn right_scrollbar_enabled(flags: &str) -> bool {
    flags.contains(['r', 'R'])
}

/// Show on interaction, hold briefly, then fade out over 400 ms.
pub fn opacity(interacting: bool, elapsed_ms: u128) -> f32 {
    if interacting {
        return 1.0;
    }
    (1.0 - elapsed_ms.saturating_sub(900) as f32 / 400.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn track() -> Rect {
        Rect {
            left: 90.0,
            right: 100.0,
            top: 20.0,
            bottom: 220.0,
        }
    }
    #[test]
    fn small_buffers_have_no_scrollbar() {
        for total in [0, 1, 10] {
            assert!(VerticalScrollbar::new(track(), total, 10, 0, 24.0).is_none());
        }
    }
    #[test]
    fn thumb_is_proportional_and_reaches_both_ends() {
        let first = VerticalScrollbar::new(track(), 100, 20, 0, 24.0).unwrap();
        assert_eq!(first.thumb.height(), 40.0);
        assert_eq!(first.thumb.top, 20.0);
        let last = VerticalScrollbar::new(track(), 100, 20, 80, 24.0).unwrap();
        assert_eq!(last.thumb.bottom, 220.0);
    }
    #[test]
    fn dragging_retains_grab_offset_and_clamps() {
        let bar = VerticalScrollbar::new(track(), 100, 20, 40, 24.0).unwrap();
        assert_eq!(bar.top_for_pointer(bar.thumb.top + 7.0, 7.0), 40);
        assert_eq!(bar.top_for_pointer(-100.0, 7.0), 0);
        assert_eq!(bar.top_for_pointer(1000.0, 7.0), 80);
        assert_eq!(bar.top_for_pointer(120.0, 20.0), 40);
    }
    #[test]
    fn minimum_thumb_remains_draggable_in_short_panes() {
        let rect = Rect {
            bottom: 30.0,
            ..track()
        };
        let bar = VerticalScrollbar::new(rect, 100_000, 1, 0, 24.0).unwrap();
        assert!(bar.thumb.height() < rect.height());
        assert_eq!(bar.top_for_pointer(100.0, 0.0), 99_999);
    }
    #[test]
    fn resize_and_stale_top_are_clamped() {
        let bar = VerticalScrollbar::new(track(), 100, 90, usize::MAX, 24.0).unwrap();
        assert_eq!(bar.thumb.bottom, bar.track.bottom);
        assert!(
            VerticalScrollbar::new(
                Rect {
                    right: 90.0,
                    ..track()
                },
                100,
                10,
                0,
                24.0
            )
            .is_none()
        );
    }
    #[test]
    fn hit_testing_excludes_status_and_adjacent_panes() {
        assert!(track().contains(95.0, 21.0));
        assert!(!track().contains(89.0, 21.0));
        assert!(!track().contains(95.0, 220.0));
    }
    #[test]
    fn guioptions_control_right_scrollbar() {
        assert!(right_scrollbar_enabled("egmrLtT"));
        assert!(right_scrollbar_enabled("R"));
        assert!(!right_scrollbar_enabled("eL"));
    }
    #[test]
    fn fade_holds_then_fades_and_hover_overrides_idle() {
        assert_eq!(opacity(false, 500), 1.0);
        assert_eq!(opacity(false, 1100), 0.5);
        assert_eq!(opacity(false, 1500), 0.0);
        assert_eq!(opacity(true, 5000), 1.0);
    }
}
