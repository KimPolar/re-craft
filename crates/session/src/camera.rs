use glam::{DVec3, Mat4, Vec3};

/// Fixed-tick camera inputs, kept separate from collision and interaction state.
#[derive(Clone, Copy, Debug)]
pub struct CameraMotion {
    previous_position: DVec3,
    position: DVec3,
    previous_eye_height: f32,
    eye_height: f32,
    previous_fov_modifier: f32,
    fov_modifier: f32,
    previous_walk_distance: f32,
    walk_distance: f32,
    previous_bob: f32,
    bob: f32,
}

impl CameraMotion {
    pub fn new(position: DVec3, eye_height: f32) -> Self {
        Self {
            previous_position: position,
            position,
            previous_eye_height: eye_height,
            eye_height,
            previous_fov_modifier: 1.0,
            fov_modifier: 1.0,
            previous_walk_distance: 0.0,
            walk_distance: 0.0,
            previous_bob: 0.0,
            bob: 0.0,
        }
    }

    /// Advance once for every simulation tick, including ticks without a frame.
    pub fn tick(
        &mut self,
        position: DVec3,
        target_eye_height: f32,
        sprinting: bool,
        flying: bool,
        effect_scale: f32,
    ) {
        let movement_speed = if sprinting {
            (0.1_f64 * 1.3) as f32
        } else {
            0.1_f32
        };
        self.tick_with_speed(
            position,
            target_eye_height,
            movement_speed,
            0.1_f32,
            flying,
            effect_scale,
        );
    }

    pub fn tick_with_speed(
        &mut self,
        position: DVec3,
        target_eye_height: f32,
        movement_speed: f32,
        walking_speed: f32,
        flying: bool,
        effect_scale: f32,
    ) {
        self.previous_position = self.position;
        self.position = position;
        self.previous_eye_height = self.eye_height;
        self.eye_height += (target_eye_height - self.eye_height) * 0.5;
        self.previous_fov_modifier = self.fov_modifier;
        let mut modifier = if flying { 1.1_f32 } else { 1.0_f32 };
        if walking_speed != 0.0 {
            modifier *= (movement_speed / walking_speed + 1.0) / 2.0;
        }
        let target = 1.0_f32 + effect_scale * (modifier - 1.0);
        self.fov_modifier += (target - self.fov_modifier) * 0.5;
        self.fov_modifier = self.fov_modifier.clamp(0.1, 1.5);
    }

    /// Translate both endpoints when a streamed world's local origin changes.
    pub(crate) fn rebase(&mut self, offset: DVec3) {
        self.previous_position += offset;
        self.position += offset;
    }

    pub(crate) fn snap_position(&mut self, position: DVec3) {
        self.previous_position = position;
        self.position = position;
    }

    /// Java updates bob from the velocity and contact state before player movement.
    pub fn begin_bob_tick(&mut self, velocity: DVec3, grounded_and_alive: bool) {
        self.previous_walk_distance = self.walk_distance;
        self.previous_bob = self.bob;
        let target = if grounded_and_alive {
            (velocity.x * velocity.x + velocity.z * velocity.z).sqrt() as f32
        } else {
            0.0
        }
        .min(0.1);
        self.bob += (target - self.bob) * 0.4;
    }

    /// Accumulate the actual, collision-clipped horizontal movement.
    pub fn finish_bob_tick(&mut self, displacement: DVec3) {
        let x = displacement.x as f32;
        let z = displacement.z as f32;
        self.walk_distance += f64::from(x * x + z * z).sqrt() as f32 * 0.6;
    }

    /// Vanilla's walking transform is applied to both the scene and held items.
    pub fn bob_transform(&self, partial_tick: f32) -> Mat4 {
        let fraction = partial_tick.clamp(0.0, 1.0);
        let walk =
            -(self.walk_distance + (self.walk_distance - self.previous_walk_distance) * fraction);
        let bob = self.previous_bob + fraction * (self.bob - self.previous_bob);
        let phase = walk * std::f32::consts::PI;
        let sin = crate::physics::java_sin(f64::from(phase), false);
        let cos = crate::physics::java_sin(f64::from(phase), true);
        let pitched = crate::physics::java_sin(f64::from(phase - 0.2), true);
        Mat4::from_translation(Vec3::new(sin * bob * 0.5, -(cos * bob).abs(), 0.0))
            * Mat4::from_rotation_z((sin * bob * 3.0).to_radians())
            * Mat4::from_rotation_x(((pitched * bob).abs() * 5.0).to_radians())
    }

    /// Match the Java camera's double-precision position and float eye lerps.
    pub fn eye(&self, partial_tick: f32) -> DVec3 {
        let fraction = f64::from(partial_tick.clamp(0.0, 1.0));
        let position = self.previous_position + (self.position - self.previous_position) * fraction;
        let eye_height = self.previous_eye_height
            + partial_tick.clamp(0.0, 1.0) * (self.eye_height - self.previous_eye_height);
        position + DVec3::Y * f64::from(eye_height)
    }

    pub fn fov_modifier(&self, partial_tick: f32) -> f32 {
        self.previous_fov_modifier
            + partial_tick.clamp(0.0, 1.0) * (self.fov_modifier - self.previous_fov_modifier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CROUCH_EYE_HEIGHT, EYE_HEIGHT, GameSession, LocalSession, PlayerCommand, PlayerPose,
    };
    use std::time::Duration;

    #[test]
    fn camera_moves_between_ticks_independently_of_frame_durations() {
        let mut sessions = [
            LocalSession::demo(),
            LocalSession::demo(),
            LocalSession::demo(),
        ];
        for session in &mut sessions {
            for _ in 0..20 {
                session.update(Duration::from_millis(50));
            }
            session.submit_command(PlayerCommand::Move {
                forward: 1.0,
                strafe: 0.0,
                jump: false,
            });
        }
        sessions[0].update(Duration::from_millis(175));
        for _ in 0..7 {
            sessions[1].update(Duration::from_millis(25));
        }
        for _ in 0..35 {
            sessions[2].update(Duration::from_millis(5));
        }
        let snapshots = sessions.each_ref().map(GameSession::snapshot);
        assert!(snapshots.iter().all(|snapshot| snapshot.tick == 23));
        let start = snapshots[0].camera.eye(0.0);
        let end = snapshots[0].camera.eye(1.0);
        let middle = snapshots[0].camera.eye(sessions[0].partial_tick());
        assert_ne!(
            snapshots[0]
                .camera
                .bob_transform(sessions[0].partial_tick()),
            Mat4::IDENTITY
        );
        assert!((end - start).length() > 0.001);
        assert!((middle - (start + end) * 0.5).length() < 1.0e-7);
        for (snapshot, session) in snapshots.iter().zip(&sessions) {
            assert!((snapshot.camera.eye(session.partial_tick()) - middle).length() < 1.0e-12);
            assert_eq!(
                snapshot.camera.bob_transform(session.partial_tick()),
                snapshots[0]
                    .camera
                    .bob_transform(sessions[0].partial_tick())
            );
        }
        sessions[0].update(Duration::from_millis(50));
        assert_eq!(snapshots[0].camera.eye(0.5), middle);
    }

    #[test]
    fn walking_bob_rises_on_ground_and_fades_in_air() {
        let mut camera = CameraMotion::new(DVec3::ZERO, 1.62);
        for _ in 0..20 {
            camera.begin_bob_tick(DVec3::new(0.1, 0.0, 0.0), true);
            camera.finish_bob_tick(DVec3::new(0.1, 0.0, 0.0));
        }
        assert_ne!(camera.bob_transform(1.0), Mat4::IDENTITY);
        for _ in 0..100 {
            camera.begin_bob_tick(DVec3::new(0.1, 0.0, 0.0), false);
            camera.finish_bob_tick(DVec3::new(0.1, 0.0, 0.0));
        }
        let difference = camera.bob_transform(1.0) - Mat4::IDENTITY;
        assert!(
            difference
                .to_cols_array()
                .iter()
                .all(|value| value.abs() < 1.0e-8)
        );
    }

    #[test]
    fn rebasing_preserves_both_camera_endpoints() {
        let origin = DVec3::new(16_000_000.0, 0.0, -16_000_000.0);
        let mut camera = CameraMotion::new(DVec3::new(0.125, 60.0, 15.875), 1.62);
        camera.tick(DVec3::new(0.2, 60.25, 16.025), 1.27, false, false, 1.0);
        let before = [0.0, 0.25, 0.5, 1.0].map(|partial| camera.eye(partial) + origin);
        let offset = DVec3::new(-16.0, 0.0, 16.0);
        camera.rebase(offset);
        let new_origin = origin - offset;
        for (partial, expected) in [0.0, 0.25, 0.5, 1.0].into_iter().zip(before) {
            assert_eq!(camera.eye(partial) + new_origin, expected);
        }
    }

    #[test]
    fn eye_height_tracks_the_pose_from_before_the_player_tick() {
        let mut session = LocalSession::demo();
        for _ in 0..20 {
            session.update(Duration::from_millis(50));
        }
        assert_eq!(session.snapshot().player.pose, PlayerPose::Standing);
        session.submit_command(PlayerCommand::Sneak(true));
        session.update(Duration::from_millis(50));
        let crouching = session.snapshot();
        assert_eq!(crouching.player.pose, PlayerPose::Crouching);
        let eye_offset = crouching.camera.eye(1.0).y - session.motion.position.y;
        assert!((eye_offset - f64::from(EYE_HEIGHT)).abs() < 1.0e-7);
        session.update(Duration::from_millis(50));
        let next = session.snapshot();
        let eye_offset = next.camera.eye(1.0).y - session.motion.position.y;
        assert!(
            (eye_offset - f64::from(EYE_HEIGHT + (CROUCH_EYE_HEIGHT - EYE_HEIGHT) * 0.5)).abs()
                < 1.0e-7
        );
    }

    #[test]
    fn flight_fov_uses_previous_tick_state_and_applies_effect_setting() {
        let mut session = LocalSession::demo();
        session.set_game_mode(crate::GameMode::Spectator);
        session.update(crate::TICK);
        assert!((session.snapshot().camera.fov_modifier(1.0) - 1.05).abs() < 1e-6);
        session.set_fov_effect_scale(0.0);
        session.update(crate::TICK);
        assert!((session.snapshot().camera.fov_modifier(1.0) - 1.025).abs() < 1e-6);
        for _ in 0..20 {
            session.update(crate::TICK);
        }
        assert!((session.snapshot().camera.fov_modifier(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn walking_speed_scales_field_of_view_and_zero_avoids_division() {
        let mut camera = CameraMotion::new(DVec3::ZERO, 1.62);
        camera.tick_with_speed(DVec3::ZERO, 1.62, 0.25, 0.25, false, 1.0);
        assert_eq!(camera.fov_modifier(1.0), 1.0);
        camera.tick_with_speed(DVec3::ZERO, 1.62, 0.5, 0.25, false, 1.0);
        assert_eq!(camera.fov_modifier(1.0), 1.25);
        let mut zero = CameraMotion::new(DVec3::ZERO, 1.62);
        zero.tick_with_speed(DVec3::ZERO, 1.62, 0.5, 0.0, true, 1.0);
        assert!((zero.fov_modifier(1.0) - 1.05).abs() < 1e-6);
    }

    #[test]
    fn sprint_fov_tracks_simulation_state_on_the_following_camera_tick() {
        let mut session = LocalSession::demo();
        for _ in 0..20 {
            session.update(Duration::from_millis(50));
        }
        session.submit_command(PlayerCommand::Move {
            forward: 1.0,
            strafe: 0.0,
            jump: false,
        });
        session.submit_command(PlayerCommand::Sprint {
            pressed: true,
            backward_pressed: false,
        });
        session.update(Duration::from_millis(50));
        let starting = session.snapshot();
        assert!(starting.sprinting);
        assert_eq!(starting.camera.fov_modifier(1.0), 1.0);
        session.update(Duration::from_millis(50));
        let running = session.snapshot();
        assert!(running.camera.fov_modifier(1.0) > 1.0);
        assert_eq!(running.camera.fov_modifier(0.0), 1.0);
        session.submit_command(PlayerCommand::Move {
            forward: 0.0,
            strafe: 0.0,
            jump: false,
        });
        session.update(Duration::from_millis(50));
        let stopping = session.snapshot();
        assert!(!stopping.sprinting);
        session.update(Duration::from_millis(50));
        assert!(session.snapshot().camera.fov_modifier(1.0) < stopping.camera.fov_modifier(1.0));
    }
}
