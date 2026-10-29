use glam::Vec3;
use native_session::WorldSnapshot;

/// Show locally received mouse motion before its command reaches the next tick.
/// The authoritative simulation still applies the commands in their original order.
#[derive(Default)]
pub struct VisualLook {
    tick: Option<u64>,
    yaw: f32,
    pitch: f32,
}

impl VisualLook {
    fn synchronize(&mut self, snapshot: &WorldSnapshot) {
        if self.tick != Some(snapshot.tick) {
            self.tick = Some(snapshot.tick);
            self.yaw = snapshot.player.yaw;
            self.pitch = snapshot.player.pitch;
        }
    }

    pub fn turn(&mut self, snapshot: &WorldSnapshot, yaw: f32, pitch: f32) {
        self.synchronize(snapshot);
        if yaw.is_finite() && pitch.is_finite() {
            self.yaw += yaw;
            self.pitch = (self.pitch + pitch).clamp(-1.54, 1.54);
        }
    }

    pub fn angles(&mut self, snapshot: &WorldSnapshot) -> (f32, f32) {
        self.synchronize(snapshot);
        (self.yaw, self.pitch)
    }

    pub fn direction(&mut self, snapshot: &WorldSnapshot) -> Vec3 {
        let (yaw, pitch) = self.angles(snapshot);
        let horizontal = pitch.cos();
        Vec3::new(yaw.sin() * horizontal, pitch.sin(), -yaw.cos() * horizontal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_session::{GameSession, LocalSession, PlayerCommand};
    use std::time::Duration;

    #[test]
    fn mouse_is_visible_before_the_tick_and_matches_the_applied_commands() {
        let mut session = LocalSession::demo();
        let mut look = VisualLook::default();
        let before = session.snapshot();
        for (yaw, pitch) in [(0.2, 1.7), (0.1, -0.2), (-0.05, 0.1)] {
            look.turn(&before, yaw, pitch);
            session.submit_command(PlayerCommand::Look { yaw, pitch });
        }
        let predicted = look.angles(&before);
        assert_ne!(predicted, (before.player.yaw, before.player.pitch));
        session.update(Duration::from_millis(49));
        assert_eq!(look.angles(&session.snapshot()), predicted);
        session.update(Duration::from_millis(1));
        let after = session.snapshot();
        assert_eq!(predicted, (after.player.yaw, after.player.pitch));
        assert_eq!(look.angles(&after), predicted);
        assert_eq!(look.direction(&after), after.player.direction());
        look.turn(&after, 0.3, f32::NAN);
        assert_eq!(look.angles(&after), predicted);
    }
}
