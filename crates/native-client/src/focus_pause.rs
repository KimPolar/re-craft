use std::time::{Duration, Instant};

/// Java's default pauseOnLostFocus waits until the window has been inactive
/// for more than 500 ms. An existing screen must never be replaced.
#[derive(Default)]
pub(crate) struct FocusPause {
    lost_at: Option<Instant>,
    debug_key_down: bool,
}

impl FocusPause {
    pub fn focus_changed(&mut self, focused: bool, now: Instant) {
        if focused {
            self.lost_at = None;
        } else if self.lost_at.is_none() {
            self.lost_at = Some(now);
        }
        if !focused {
            self.debug_key_down = false;
        }
    }

    /// Releases still update the modifier while another screen is open so it
    /// cannot get stuck. Java processes both press and repeat actions.
    pub fn key(
        &mut self,
        code: winit::keyboard::KeyCode,
        pressed: bool,
        screen_open: bool,
    ) -> bool {
        use winit::keyboard::KeyCode;
        if code == KeyCode::F3 {
            self.debug_key_down = pressed;
            return false;
        }
        code == KeyCode::KeyP && pressed && self.debug_key_down && !screen_open
    }

    pub fn should_pause(&self, now: Instant, screen_open: bool, enabled: bool) -> bool {
        enabled
            && !screen_open
            && self.lost_at.is_some_and(|lost| {
                now.saturating_duration_since(lost) > Duration::from_millis(500)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_for_focus_loss_and_never_replaces_an_open_screen() {
        let start = Instant::now();
        let mut focus = FocusPause::default();
        assert!(!focus.should_pause(start + Duration::from_secs(2), false, true));
        focus.focus_changed(false, start);
        assert!(!focus.should_pause(start + Duration::from_millis(500), false, true));
        assert!(focus.should_pause(start + Duration::from_millis(501), false, true));
        assert!(!focus.should_pause(start + Duration::from_secs(2), true, true));
        assert!(!focus.should_pause(start + Duration::from_secs(2), false, false));
        assert!(focus.should_pause(start + Duration::from_secs(2), false, true));
        focus.focus_changed(true, start + Duration::from_secs(2));
        assert!(!focus.should_pause(start + Duration::from_secs(3), false, true));
        focus.focus_changed(false, start + Duration::from_secs(3));
        focus.focus_changed(false, start + Duration::from_millis(3200));
        assert!(focus.should_pause(start + Duration::from_millis(3501), false, true));
    }

    #[test]
    fn debug_chord_tracks_modifier_and_clears_on_focus_loss() {
        use winit::keyboard::KeyCode::{F3, KeyP};
        let mut focus = FocusPause::default();
        assert!(!focus.key(KeyP, true, false));
        assert!(!focus.key(F3, true, false));
        assert!(focus.key(KeyP, true, false));
        assert!(!focus.key(KeyP, true, true));
        focus.focus_changed(false, Instant::now());
        assert!(!focus.key(KeyP, true, false));
    }
}
