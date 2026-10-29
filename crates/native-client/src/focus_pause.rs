use std::time::{Duration, Instant};

/// Java's default pauseOnLostFocus waits until the window has been inactive
/// for more than 500 ms. An existing screen must never be replaced.
#[derive(Default)]
pub(crate) struct FocusPause {
    lost_at: Option<Instant>,
}

impl FocusPause {
    pub fn focus_changed(&mut self, focused: bool, now: Instant) {
        if focused {
            self.lost_at = None;
        } else if self.lost_at.is_none() {
            self.lost_at = Some(now);
        }
    }

    pub fn should_pause(&self, now: Instant, screen_open: bool) -> bool {
        !screen_open
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
        assert!(!focus.should_pause(start + Duration::from_secs(2), false));
        focus.focus_changed(false, start);
        assert!(!focus.should_pause(start + Duration::from_millis(500), false));
        assert!(focus.should_pause(start + Duration::from_millis(501), false));
        assert!(!focus.should_pause(start + Duration::from_secs(2), true));
        assert!(focus.should_pause(start + Duration::from_secs(2), false));
        focus.focus_changed(true, start + Duration::from_secs(2));
        assert!(!focus.should_pause(start + Duration::from_secs(3), false));
        focus.focus_changed(false, start + Duration::from_secs(3));
        focus.focus_changed(false, start + Duration::from_millis(3200));
        assert!(focus.should_pause(start + Duration::from_millis(3501), false));
    }
}
