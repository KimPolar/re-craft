//! Default player movement in air, on ordinary solid blocks, and in fluids.
//! Some status effects and custom attributes are handled by the session.

use glam::DVec3;
use native_world::fluid::FluidKind;

#[derive(Clone, Copy, Debug)]
pub struct MovementInput {
    pub forward: f32,
    /// Positive values move to the player's right.
    pub right: f32,
    /// Java yaw in degrees (0 faces positive Z).
    pub yaw_degrees: f32,
    pub jump: bool,
    pub sprinting: bool,
    pub using_item: bool,
    /// The client applies the sneaking-speed attribute to input before movement.
    pub moving_slowly: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct MotionState {
    pub position: DVec3,
    /// Blocks per 20 TPS tick.
    pub velocity: DVec3,
    pub on_ground: bool,
    pub jump_delay: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct FluidMovement {
    pub kind: FluidKind,
    pub depth: f64,
    pub descend: bool,
    pub swimming: bool,
    pub look_y: f64,
    pub fluid_above: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WaterMovement {
    pub efficiency: f32,
    pub movement_speed: f32,
    pub dolphins_grace: bool,
}

impl Default for WaterMovement {
    fn default() -> Self {
        Self {
            efficiency: 0.0,
            movement_speed: 0.1,
            dolphins_grace: false,
        }
    }
}

/// Java computes the jump impulse in float precision after reading attributes.
pub fn jump_power(strength: f64, block_factor: f32, boost: Option<u8>) -> f32 {
    let boost = boost.map_or(0.0, |amplifier| 0.1_f32 * (f32::from(amplifier) + 1.0));
    strength as f32 * block_factor + boost
}

/// LivingEntity.getEffectiveGravity uses the velocity at the point of travel.
fn effective_gravity(gravity: f64, slow_falling: bool, vertical_velocity: f64) -> f64 {
    if slow_falling && vertical_velocity <= 0.0 {
        gravity.min(0.01)
    } else {
        gravity
    }
}

/// Vanilla block jump factors are independent of block state.
pub fn block_jump_factor(block: &str) -> f32 {
    if block == "minecraft:honey_block" {
        0.5
    } else {
        1.0
    }
}

/// Vanilla's horizontal movement multiplier before movement efficiency.
pub fn block_speed_factor(block: &str) -> f32 {
    match block {
        "minecraft:honey_block" | "minecraft:soul_sand" => 0.4,
        _ => 1.0,
    }
}

/// Water and bubble columns prevent the block underneath from slowing the entity.
pub fn effective_block_speed_factor(at_feet: &str, below: &str, efficiency: f64) -> f32 {
    let at_feet_factor = block_speed_factor(at_feet);
    let factor = if at_feet_factor == 1.0
        && !matches!(at_feet, "minecraft:water" | "minecraft:bubble_column")
    {
        block_speed_factor(below)
    } else {
        at_feet_factor
    };
    factor + (efficiency as f32) * (1.0 - factor)
}

fn default_movement_speed(sprinting: bool) -> f32 {
    if sprinting {
        (f64::from(0.1_f32) * (1.0 + f64::from(0.3_f32))) as f32
    } else {
        0.1_f32
    }
}

/// Minecraft applies the accumulated vertical wheel count in float precision.
pub fn adjust_flight_speed(speed: f32, steps: i32) -> f32 {
    (speed + steps as f32 * 0.005_f32).clamp(0.0, 0.2)
}

/// Client flight toggle, including Java's seven-tick double-jump window.
#[derive(Default)]
pub struct FlightState {
    pub flying: bool,
    previous_jump: bool,
    trigger_time: u8,
}

impl FlightState {
    /// Returns true when flight started on the ground and needs a jump impulse.
    pub fn update(
        &mut self,
        mode: crate::GameMode,
        jump: bool,
        swimming: bool,
        on_ground: bool,
    ) -> bool {
        let mut start_jump = false;
        if mode == crate::GameMode::Spectator {
            self.flying = true;
        } else if mode == crate::GameMode::Creative {
            if jump && !self.previous_jump {
                if self.trigger_time == 0 {
                    self.trigger_time = 7;
                } else if !swimming {
                    self.flying = !self.flying;
                    start_jump = self.flying && on_ground;
                    self.trigger_time = 0;
                }
            }
        } else {
            self.flying = false;
        }
        self.previous_jump = jump;
        self.trigger_time = self.trigger_time.saturating_sub(1);
        start_jump
    }

    pub fn after_movement(&mut self, mode: crate::GameMode, on_ground: bool) {
        if mode != crate::GameMode::Spectator && on_ground {
            self.flying = false;
        }
    }
}

/// The modified friction returned by 26.3 for the supported vanilla blocks.
pub fn ground_friction(block: &str) -> f32 {
    match block {
        "minecraft:ice" | "minecraft:packed_ice" | "minecraft:frosted_ice" => 0.98,
        "minecraft:blue_ice" => 0.989,
        "minecraft:slime_block" => 0.8,
        _ => 0.6,
    }
}

impl MotionState {
    /// Default spectator flight in air, with the player's no-physics movement.
    pub fn tick_spectator(
        &mut self,
        input: MovementInput,
        descend: bool,
        flight_speed: f32,
    ) -> DVec3 {
        self.on_ground = false;
        self.tick_flying(input, descend, flight_speed, 0.6, |_, requested, _| {
            (requested, requested)
        })
        .1
    }

    /// Player flight uses ordinary collision while retaining vertical momentum.
    pub fn tick_flying(
        &mut self,
        input: MovementInput,
        descend: bool,
        flight_speed: f32,
        block_friction: f32,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
    ) -> (bool, DVec3) {
        self.tick_flying_with_bounciness(input, descend, flight_speed, block_friction, 0.0, collide)
    }

    /// Flight preserves the pre-collision vertical velocity but reflects blocked horizontal motion.
    pub fn tick_flying_with_bounciness(
        &mut self,
        input: MovementInput,
        descend: bool,
        flight_speed: f32,
        block_friction: f32,
        bounciness: f64,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
    ) -> (bool, DVec3) {
        self.tick_flying_with_movement_speed(
            input,
            descend,
            flight_speed,
            block_friction,
            bounciness,
            default_movement_speed(input.sprinting),
            collide,
        )
    }

    pub(crate) fn tick_flying_with_movement_speed(
        &mut self,
        input: MovementInput,
        descend: bool,
        flight_speed: f32,
        block_friction: f32,
        bounciness: f64,
        movement_speed: f32,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
    ) -> (bool, DVec3) {
        let vertical = i32::from(input.jump) - i32::from(descend);
        self.velocity.y += f64::from(vertical as f32 * flight_speed * 3.0_f32);
        // Player.isAffectedByFluids() is false while flying. LivingEntity.aiStep
        // consequently skips its ordinary jump handling and resets this delay.
        self.jump_delay = 0;
        if self.velocity.x * self.velocity.x + self.velocity.z * self.velocity.z < 9.0e-6 {
            self.velocity.x = 0.0;
            self.velocity.z = 0.0;
        }
        if self.velocity.y.abs() < 0.003 {
            self.velocity.y = 0.0;
        }
        let radians = input.yaw_degrees * 0.017453292_f32;
        let sine = f64::from(java_sin(f64::from(radians), false));
        let cosine = f64::from(java_sin(f64::from(radians), true));
        let friction = if self.on_ground {
            modified_friction(block_friction, 1.0)
        } else {
            1.0
        };
        let speed = if self.on_ground {
            if f64::from(friction) > 0.6 {
                movement_speed * (0.21600002_f32 / (friction * friction * friction))
            } else {
                movement_speed
            }
        } else if input.sprinting {
            flight_speed * 2.0_f32
        } else {
            flight_speed
        };
        let (left, forward) = keyboard_input(
            -input.right,
            input.forward,
            input.moving_slowly,
            input.using_item,
        );
        let x = f64::from(left) * f64::from(speed);
        let z = f64::from(forward) * f64::from(speed);
        self.velocity.x += x * cosine - z * sine;
        self.velocity.z += z * cosine + x * sine;
        let vertical_velocity = self.velocity.y;
        let (requested, actual) = collide(self.position, self.velocity, self.on_ground);
        self.position += actual;
        let epsilon = f64::from(1.0e-5_f32);
        let blocked_x = (requested.x - actual.x).abs() >= epsilon;
        let blocked_z = (requested.z - actual.z).abs() >= epsilon;
        self.on_ground = requested.y != actual.y && requested.y < 0.0;
        if blocked_x {
            self.velocity.x *= -bounciness;
        }
        if blocked_z {
            self.velocity.z *= -bounciness;
        }
        let horizontal_drag = friction * modified_friction(0.91, 1.0);
        self.velocity.x *= f64::from(horizontal_drag);
        self.velocity.y = vertical_velocity * 0.6;
        self.velocity.z *= f64::from(horizontal_drag);
        (
            (blocked_x || blocked_z) && !minor_collision(left, forward, sine, cosine, actual),
            actual,
        )
    }

    fn jump_from_ground(&mut self, sprinting: bool, sine: f64, cosine: f64, power: f32) {
        if power <= 1.0e-5_f32 {
            return;
        }
        self.velocity.y = self.velocity.y.max(f64::from(power));
        if sprinting {
            self.velocity.x -= sine * 0.2;
            self.velocity.z += cosine * 0.2;
        }
    }

    pub fn start_flying_jump(&mut self, input: MovementInput) {
        self.start_flying_jump_with_power(input, 0.42_f32);
    }

    pub fn start_flying_jump_with_power(&mut self, input: MovementInput, power: f32) {
        let radians = input.yaw_degrees * 0.017453292_f32;
        self.jump_from_ground(
            input.sprinting,
            f64::from(java_sin(f64::from(radians), false)),
            f64::from(java_sin(f64::from(radians), true)),
            power,
        );
    }

    /// Advance one 20 TPS tick. `collide` resolves the requested movement and
    /// returns the movement that is actually possible from the given position.
    /// Returns whether a non-minor horizontal collision occurred.
    pub fn tick(
        &mut self,
        input: MovementInput,
        block_friction: f32,
        collide: impl FnOnce(DVec3, DVec3, bool) -> DVec3,
    ) -> bool {
        self.tick_resolved(input, block_friction, |position, requested, on_ground| {
            (requested, collide(position, requested, on_ground))
        })
        .0
    }

    /// Resolves movement after optional edge restraint. The first returned vector
    /// is the adjusted request, the second is the collision-clipped movement.
    pub fn tick_resolved(
        &mut self,
        input: MovementInput,
        block_friction: f32,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
    ) -> (bool, DVec3) {
        self.tick_resolved_with_power(input, block_friction, 0.42_f32, collide)
    }

    pub fn tick_resolved_with_power(
        &mut self,
        input: MovementInput,
        block_friction: f32,
        jump_power: f32,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
    ) -> (bool, DVec3) {
        self.tick_resolved_with_effects(
            input,
            block_friction,
            default_movement_speed(input.sprinting),
            jump_power,
            0.08,
            false,
            None,
            collide,
        )
    }

    pub fn tick_resolved_with_effects(
        &mut self,
        input: MovementInput,
        block_friction: f32,
        movement_speed: f32,
        jump_power: f32,
        gravity: f64,
        slow_falling: bool,
        levitation: Option<u8>,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
    ) -> (bool, DVec3) {
        self.tick_resolved_with_restitution(
            input,
            block_friction,
            movement_speed,
            jump_power,
            gravity,
            slow_falling,
            levitation,
            collide,
            |_, _, _| (0.0, 0.0),
            |_, _, _| 1.0,
        )
    }

    pub(crate) fn tick_resolved_with_restitution(
        &mut self,
        input: MovementInput,
        block_friction: f32,
        movement_speed: f32,
        jump_power: f32,
        gravity: f64,
        slow_falling: bool,
        levitation: Option<u8>,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
        restitution: impl FnOnce(DVec3, DVec3, DVec3) -> (f64, f64),
        block_speed: impl FnOnce(DVec3, DVec3, bool) -> f32,
    ) -> (bool, DVec3) {
        let radians = input.yaw_degrees * 0.017453292_f32;
        let sine = f64::from(java_sin(f64::from(radians), false));
        let cosine = f64::from(java_sin(f64::from(radians), true));
        self.jump_delay = self.jump_delay.saturating_sub(1);
        if self.velocity.x * self.velocity.x + self.velocity.z * self.velocity.z < 9.0e-6 {
            self.velocity.x = 0.0;
            self.velocity.z = 0.0;
        }
        if self.velocity.y.abs() < 0.003 {
            self.velocity.y = 0.0;
        }
        if input.jump {
            if self.on_ground && self.jump_delay == 0 {
                self.jump_from_ground(input.sprinting, sine, cosine, jump_power);
                self.jump_delay = 10;
            }
        } else {
            self.jump_delay = 0;
        }

        let friction = if self.on_ground {
            modified_friction(block_friction, 1.0)
        } else {
            1.0
        };
        let speed = if self.on_ground {
            if f64::from(friction) > 0.6 {
                movement_speed * (0.21600002_f32 / (friction * friction * friction))
            } else {
                movement_speed
            }
        } else if input.sprinting {
            0.02_f32 * 1.3_f32
        } else {
            0.02_f32
        };
        let (left, forward) = keyboard_input(
            -input.right,
            input.forward,
            input.moving_slowly,
            input.using_item,
        );
        let x = f64::from(left) * f64::from(speed);
        let z = f64::from(forward) * f64::from(speed);
        self.velocity.x += x * cosine - z * sine;
        self.velocity.z += z * cosine + x * sine;

        let (requested, actual) = collide(self.position, self.velocity, self.on_ground);
        self.position += actual;
        let horizontal_epsilon = f64::from(1.0e-5_f32);
        let blocked_x = (requested.x - actual.x).abs() >= horizontal_epsilon;
        let blocked_z = (requested.z - actual.z).abs() >= horizontal_epsilon;
        let vertical_collision = requested.y != actual.y;
        self.on_ground = vertical_collision && requested.y < 0.0;
        let (horizontal_restitution, vertical_velocity) =
            if blocked_x || blocked_z || vertical_collision {
                restitution(self.position, requested, actual)
            } else {
                (0.0, 0.0)
            };
        if blocked_x {
            self.velocity.x *= -horizontal_restitution;
        }
        if blocked_z {
            self.velocity.z *= -horizontal_restitution;
        }
        if vertical_collision {
            self.velocity.y = vertical_velocity;
        }
        let block_speed = f64::from(block_speed(self.position, actual, self.on_ground));
        self.velocity.x *= block_speed;
        self.velocity.z *= block_speed;
        let horizontal_drag = friction * modified_friction(0.91, 1.0);
        let vertical_drag = modified_friction(0.98, 1.0);
        self.velocity.x *= f64::from(horizontal_drag);
        let vertical_velocity = if let Some(amplifier) = levitation {
            self.velocity.y + (0.05 * (f64::from(amplifier) + 1.0) - self.velocity.y) * 0.2
        } else {
            let gravity = effective_gravity(gravity, slow_falling, self.velocity.y);
            self.velocity.y - gravity
        };
        self.velocity.y = vertical_velocity * f64::from(vertical_drag);
        self.velocity.z *= f64::from(horizontal_drag);
        let horizontal_collision = blocked_x || blocked_z;
        (
            horizontal_collision && !minor_collision(left, forward, sine, cosine, actual),
            actual,
        )
    }

    /// Advance default player movement in water or lava after currents are applied.
    pub fn tick_fluid(
        &mut self,
        input: MovementInput,
        fluid: FluidMovement,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
        can_jump_out: impl FnOnce(DVec3, DVec3) -> bool,
    ) -> (bool, DVec3) {
        self.tick_fluid_with_restitution(
            input,
            fluid,
            WaterMovement::default(),
            0.42_f32,
            0.08,
            false,
            collide,
            can_jump_out,
            |_, _, _| (0.0, 0.0),
            |_, _, _| 1.0,
        )
    }

    pub(crate) fn tick_fluid_with_restitution(
        &mut self,
        input: MovementInput,
        fluid: FluidMovement,
        water: WaterMovement,
        jump_power: f32,
        gravity: f64,
        slow_falling: bool,
        collide: impl FnOnce(DVec3, DVec3, bool) -> (DVec3, DVec3),
        can_jump_out: impl FnOnce(DVec3, DVec3) -> bool,
        restitution: impl FnOnce(DVec3, DVec3, DVec3) -> (f64, f64),
        block_speed: impl FnOnce(DVec3, DVec3, bool) -> f32,
    ) -> (bool, DVec3) {
        if fluid.kind == FluidKind::Water && fluid.descend {
            self.velocity.y -= f64::from(0.04_f32);
        }
        self.jump_delay = self.jump_delay.saturating_sub(1);
        if self.velocity.x * self.velocity.x + self.velocity.z * self.velocity.z < 9.0e-6 {
            self.velocity.x = 0.0;
            self.velocity.z = 0.0;
        }
        if self.velocity.y.abs() < 0.003 {
            self.velocity.y = 0.0;
        }
        let radians = input.yaw_degrees * 0.017453292_f32;
        let sine = f64::from(java_sin(f64::from(radians), false));
        let cosine = f64::from(java_sin(f64::from(radians), true));
        if input.jump {
            if fluid.depth > 0.0 && (!self.on_ground || fluid.depth > 0.4) {
                self.velocity.y += f64::from(0.04_f32);
            } else if self.on_ground && self.jump_delay == 0 {
                self.jump_from_ground(input.sprinting, sine, cosine, jump_power);
                self.jump_delay = 10;
            }
        } else {
            self.jump_delay = 0;
        }
        if fluid.kind == FluidKind::Water && fluid.swimming {
            let factor = if fluid.look_y < -0.2 { 0.085 } else { 0.06 };
            if fluid.look_y <= 0.0 || input.jump || fluid.fluid_above {
                self.velocity.y += (fluid.look_y - self.velocity.y) * factor;
            }
        }
        let falling = self.velocity.y <= 0.0;
        let gravity = effective_gravity(gravity, slow_falling, self.velocity.y);
        let mut horizontal_drag = if input.sprinting { 0.9_f32 } else { 0.8_f32 };
        let mut acceleration = 0.02_f32;
        if fluid.kind == FluidKind::Water {
            let efficiency = if self.on_ground {
                water.efficiency
            } else {
                water.efficiency * 0.5_f32
            };
            if efficiency > 0.0 {
                horizontal_drag += (0.54600006_f32 - horizontal_drag) * efficiency;
                acceleration += (water.movement_speed - acceleration) * efficiency;
            }
            if water.dolphins_grace {
                horizontal_drag = 0.96_f32;
            }
        }
        let (left, forward) = keyboard_input(
            -input.right,
            input.forward,
            input.moving_slowly,
            input.using_item,
        );
        let x = f64::from(left) * f64::from(acceleration);
        let z = f64::from(forward) * f64::from(acceleration);
        self.velocity.x += x * cosine - z * sine;
        self.velocity.z += z * cosine + x * sine;
        let previous_y = self.position.y;
        let (requested, actual) = collide(self.position, self.velocity, self.on_ground);
        self.position += actual;
        let epsilon = f64::from(1.0e-5_f32);
        let blocked_x = (requested.x - actual.x).abs() >= epsilon;
        let blocked_z = (requested.z - actual.z).abs() >= epsilon;
        let vertical_collision = requested.y != actual.y;
        self.on_ground = vertical_collision && requested.y < 0.0;
        let (horizontal_restitution, vertical_velocity) =
            if blocked_x || blocked_z || vertical_collision {
                restitution(self.position, requested, actual)
            } else {
                (0.0, 0.0)
            };
        if blocked_x {
            self.velocity.x *= -horizontal_restitution;
        }
        if blocked_z {
            self.velocity.z *= -horizontal_restitution;
        }
        if vertical_collision {
            self.velocity.y = vertical_velocity;
        }
        let block_speed = f64::from(block_speed(self.position, actual, self.on_ground));
        self.velocity.x *= block_speed;
        self.velocity.z *= block_speed;
        match fluid.kind {
            FluidKind::Water => {
                self.velocity *= DVec3::new(
                    f64::from(horizontal_drag),
                    f64::from(0.8_f32),
                    f64::from(horizontal_drag),
                );
                self.velocity = fluid_gravity(self.velocity, falling, input.sprinting, gravity);
            }
            FluidKind::Lava => {
                if fluid.depth <= 0.4 {
                    self.velocity *= DVec3::new(0.5, f64::from(0.8_f32), 0.5);
                    self.velocity = fluid_gravity(self.velocity, falling, input.sprinting, gravity);
                } else {
                    self.velocity *= 0.5;
                }
                self.velocity.y -= gravity / 4.0;
            }
        }
        let horizontal_collision = blocked_x || blocked_z;
        if horizontal_collision {
            let offset = DVec3::new(
                self.velocity.x,
                self.velocity.y + f64::from(0.6_f32) - self.position.y + previous_y,
                self.velocity.z,
            );
            if can_jump_out(self.position, offset) {
                self.velocity.y = f64::from(0.3_f32);
            }
        }
        (
            horizontal_collision && !minor_collision(left, forward, sine, cosine, actual),
            actual,
        )
    }
}

fn fluid_gravity(mut velocity: DVec3, falling: bool, sprinting: bool, gravity: f64) -> DVec3 {
    if gravity != 0.0 && !sprinting {
        velocity.y = if falling
            && (velocity.y - 0.005).abs() >= 0.003
            && (velocity.y - gravity / 16.0).abs() < 0.003
        {
            -0.003
        } else {
            velocity.y - gravity / 16.0
        };
    }
    velocity
}

/// Player look height using the same sine table as 26.3.
pub fn look_vertical(pitch_radians: f32) -> f64 {
    let java_pitch = -pitch_radians.to_degrees();
    -f64::from(java_sin(f64::from(java_pitch * 0.017453292_f32), false))
}

fn minor_collision(left: f32, forward: f32, sine: f64, cosine: f64, actual: DVec3) -> bool {
    let x = f64::from(left) * cosine - f64::from(forward) * sine;
    let z = f64::from(forward) * cosine + f64::from(left) * sine;
    let input_length = x * x + z * z;
    let actual_length = actual.x * actual.x + actual.z * actual.z;
    if input_length < f64::from(1.0e-5_f32) || actual_length < f64::from(1.0e-5_f32) {
        return false;
    }
    let angle = ((x * actual.x + z * actual.z) / (input_length * actual_length).sqrt()).acos();
    angle < f64::from(0.13962634_f32)
}

/// Client sprint controls for the supported ordinary item-use behavior.
#[derive(Default)]
pub struct SprintState {
    /// Current sprint-key state; releasing it does not itself end a run.
    pub pressed: bool,
    /// Raw backward key state, even if forward and backward cancel out.
    pub backward_pressed: bool,
    /// The previous input sample's sneak key.
    pub previous_sneak: bool,
    /// Whether the client considers the current pose slow-moving.
    pub moving_slowly: bool,
    pub in_water: bool,
    pub under_water: bool,
    pub swimming: bool,
    pub on_ground: bool,
    pub current_sneak: bool,
    /// The player cannot sprint when food is insufficient and flight is unavailable.
    pub insufficient_food: bool,
    pub using_item: bool,
    /// Sprinting state after the last input update.
    pub active: bool,
    last_forward: bool,
    trigger_time: u8,
    major_collision: bool,
}

impl SprintState {
    /// Process the current input before physics, using the previous move's collision.
    pub fn update(&mut self, forward: f32) -> bool {
        self.update_with_window(forward, 7)
    }

    /// Apply the configured double-tap window in ticks (0 disables double-tap).
    pub fn update_with_window(&mut self, forward: f32, window: u8) -> bool {
        self.trigger_time = self.trigger_time.saturating_sub(1);
        let forward_now = forward > 1.0e-5_f32;
        if self.backward_pressed || self.previous_sneak || self.using_item || forward < 0.0 {
            self.trigger_time = 0;
        }
        let sprint_possible = !self.insufficient_food && (!self.in_water || self.under_water);
        if !self.active
            && forward_now
            && sprint_possible
            && !self.using_item
            && (!self.moving_slowly || self.under_water)
        {
            if !self.last_forward {
                if self.trigger_time > 0 {
                    self.active = true;
                } else {
                    self.trigger_time = window.min(10);
                }
            }
            if self.pressed {
                self.active = true;
            }
        }
        if self.active {
            let stop = if self.swimming {
                self.insufficient_food
                    || !self.in_water
                    || (!forward_now && !self.on_ground && !self.current_sneak)
            } else {
                !sprint_possible || !forward_now || self.major_collision
            };
            if stop {
                self.active = false;
            }
        }
        self.last_forward = forward_now;
        self.active
    }

    /// Supply the collision result for the next tick's input update.
    pub fn after_move(&mut self, major_collision: bool) {
        self.major_collision = major_collision;
    }
}

fn modified_friction(friction: f32, modifier: f32) -> f32 {
    (1.0 - (1.0 - friction) * modifier).clamp(0.0, 1.0)
}

/// 26.3 normalizes the keyboard vector, scales it, then maps it to a unit square.
fn keyboard_input(left: f32, forward: f32, moving_slowly: bool, using_item: bool) -> (f32, f32) {
    let len = (left * left + forward * forward).sqrt();
    if len < 1.0e-4 {
        return (0.0, 0.0);
    }
    let mut left = (left / len) * 0.98_f32;
    let mut forward = (forward / len) * 0.98_f32;
    if using_item {
        left *= 0.2_f32;
        forward *= 0.2_f32;
    }
    if moving_slowly {
        left *= 0.3_f32;
        forward *= 0.3_f32;
    }
    let len = (left * left + forward * forward).sqrt();
    let left = left * (1.0 / len);
    let forward = forward * (1.0 / len);
    let a = left.abs();
    let b = forward.abs();
    let ratio = if b > a { a / b } else { b / a };
    let magnitude = (len * (1.0 + ratio * ratio).sqrt()).min(1.0);
    (left * magnitude, forward * magnitude)
}

pub fn java_sin(radians: f64, cosine: bool) -> f32 {
    const SINE_SCALE: f64 = 10430.378350470453;
    let index = ((radians * SINE_SCALE + if cosine { 16384.0 } else { 0.0 }) as i64) & 65535;
    ((index as f64) / SINE_SCALE).sin() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_speed_respects_feet_water_and_efficiency() {
        assert_eq!(
            effective_block_speed_factor("minecraft:air", "minecraft:soul_sand", 0.0),
            0.4
        );
        assert_eq!(
            effective_block_speed_factor("minecraft:honey_block", "minecraft:stone", 0.0),
            0.4
        );
        assert_eq!(
            effective_block_speed_factor("minecraft:water", "minecraft:soul_sand", 0.0),
            1.0
        );
        assert_eq!(
            effective_block_speed_factor("minecraft:bubble_column", "minecraft:soul_sand", 0.0),
            1.0
        );
        assert_eq!(
            effective_block_speed_factor("minecraft:air", "minecraft:soul_sand", 1.0),
            1.0
        );
        assert_eq!(
            effective_block_speed_factor("minecraft:air", "minecraft:soul_soil", 0.0),
            1.0
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn block_speed_matches_java_player() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PlayerBlockSpeedOracle.java");
        let result = temp.path().join("speeds.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PlayerBlockSpeedOracle.java"),
        )
        .unwrap();
        for output in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source)
                .output()
                .unwrap(),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("PlayerBlockSpeedOracle")
                .arg(&result)
                .output()
                .unwrap(),
        ] {
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(rows.len(), 256);
        for row in rows {
            let feet = format!("minecraft:{}", row["feet"].as_str().unwrap());
            let below = format!("minecraft:{}", row["below"].as_str().unwrap());
            let factor =
                effective_block_speed_factor(&feet, &below, row["efficiency"].as_f64().unwrap());
            assert_eq!(
                factor.to_bits(),
                row["bits"].as_i64().unwrap() as u32,
                "{row}"
            );
            if feet == "minecraft:soul_sand" {
                let state = native_world::loaded::BlockState {
                    name: feet.clone(),
                    properties: Default::default(),
                };
                let shapes = [
                    native_world::collision::block_shape(&state).unwrap(),
                    native_world::collision::block_outline(&state).unwrap(),
                ];
                for (actual, expected) in shapes.iter().zip(row["boxes"].as_array().unwrap()) {
                    let shape = actual[0];
                    let coords = [
                        shape.min.x,
                        shape.min.y,
                        shape.min.z,
                        shape.max.x,
                        shape.max.y,
                        shape.max.z,
                    ];
                    for (actual, expected) in coords.iter().zip(expected.as_array().unwrap()) {
                        assert_eq!(*actual, expected.as_f64().unwrap() as f32, "{row}");
                    }
                }
                for (direction, expected) in [
                    glam::IVec3::NEG_Y,
                    glam::IVec3::Y,
                    glam::IVec3::NEG_Z,
                    glam::IVec3::Z,
                    glam::IVec3::NEG_X,
                    glam::IVec3::X,
                ]
                .into_iter()
                .zip(row["sturdy"].as_array().unwrap())
                {
                    assert_eq!(
                        native_world::collision::button_support_face(&state, direction),
                        expected.as_bool(),
                        "{row}"
                    );
                }
            }
        }
    }

    #[test]
    fn slow_falling_gravity_uses_velocity_after_air_collision() {
        let input = MovementInput {
            forward: 0.0,
            right: 0.0,
            yaw_degrees: 0.0,
            jump: false,
            sprinting: false,
            using_item: false,
            moving_slowly: false,
        };
        for slow in [false, true] {
            let mut motion = MotionState {
                position: DVec3::ZERO,
                velocity: DVec3::new(0.0, 0.2, 0.0),
                on_ground: false,
                jump_delay: 0,
            };
            motion.tick_resolved_with_effects(
                input,
                0.6,
                0.1_f32,
                0.42_f32,
                0.08,
                slow,
                None,
                |_, requested, _| (requested, DVec3::new(requested.x, 0.0, requested.z)),
            );
            let gravity = if slow { 0.01 } else { 0.08 };
            assert_eq!(
                motion.velocity.y.to_bits(),
                (-gravity * f64::from(0.98_f32)).to_bits()
            );
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn slow_falling_gravity_matches_java() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("SlowFallingGravityOracle.java");
        let output = temp.path().join("gravity.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/SlowFallingGravityOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("SlowFallingGravityOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 144);
        for row in rows {
            let gravity = if row["no_gravity"].as_bool().unwrap() {
                0.0
            } else {
                row["gravity"].as_f64().unwrap()
            };
            let actual = effective_gravity(
                gravity,
                row["effect"].as_bool().unwrap(),
                row["vertical"].as_f64().unwrap(),
            );
            assert_eq!(
                actual.to_bits(),
                row["bits"].as_str().unwrap().parse::<i64>().unwrap() as u64,
                "{row}"
            );
        }
    }

    #[test]
    fn flying_jump_does_not_apply_ground_jump_impulse() {
        for flight_speed in [0.0_f32, 0.05] {
            let mut motion = MotionState {
                position: DVec3::ZERO,
                velocity: DVec3::ZERO,
                on_ground: true,
                jump_delay: 7,
            };
            motion.tick_flying(
                MovementInput {
                    forward: 0.0,
                    right: 0.0,
                    yaw_degrees: 0.0,
                    jump: true,
                    sprinting: false,
                    using_item: false,
                    moving_slowly: false,
                },
                false,
                flight_speed,
                0.6,
                |_, requested, _| (requested, requested),
            );
            assert_eq!(motion.position.y, f64::from(flight_speed * 3.0_f32));
            assert_eq!(motion.velocity.y, f64::from(flight_speed * 3.0_f32) * 0.6);
            assert_eq!(motion.jump_delay, 0);
        }
    }

    #[test]
    fn creative_flight_toggles_on_a_fresh_double_jump_within_the_window() {
        for gap in 1..=7 {
            let mut flight = FlightState::default();
            assert!(!flight.update(crate::GameMode::Creative, true, false, false));
            for _ in 0..gap {
                flight.update(crate::GameMode::Creative, false, false, false);
            }
            assert!(!flight.update(crate::GameMode::Creative, true, false, false));
            assert_eq!(flight.flying, gap <= 5, "gap {gap}");
        }
        let mut flight = FlightState::default();
        flight.update(crate::GameMode::Creative, true, false, false);
        for _ in 0..8 {
            flight.update(crate::GameMode::Creative, true, false, false);
        }
        assert!(!flight.flying, "holding jump is not a second press");
        flight.update(crate::GameMode::Creative, false, false, false);
        flight.update(crate::GameMode::Creative, true, false, false);
        flight.update(crate::GameMode::Creative, false, false, false);
        assert!(flight.update(crate::GameMode::Creative, true, false, true));
        assert!(flight.flying);
        flight.after_movement(crate::GameMode::Creative, true);
        assert!(!flight.flying);
        flight.update(crate::GameMode::Spectator, false, false, true);
        flight.after_movement(crate::GameMode::Spectator, true);
        assert!(flight.flying);
    }

    #[test]
    fn velocity_persists_when_input_is_released_and_ice_reduces_drag() {
        let initial = MotionState {
            position: DVec3::ZERO,
            velocity: DVec3::new(0.0, 0.0, 0.2),
            on_ground: true,
            jump_delay: 0,
        };
        let input = MovementInput {
            forward: 0.0,
            right: 0.0,
            yaw_degrees: 0.0,
            jump: false,
            sprinting: false,
            using_item: false,
            moving_slowly: false,
        };
        let mut stone = initial;
        let mut ice = initial;
        stone.tick(input, 0.6, |_, displacement, _| displacement);
        ice.tick(input, 0.98, |_, displacement, _| displacement);
        assert!(ice.velocity.z > stone.velocity.z);
        let previous = stone.position.z;
        stone.tick(
            MovementInput {
                forward: 0.0,
                ..input
            },
            0.6,
            |_, displacement, _| displacement,
        );
        assert!(stone.position.z > previous);
    }

    #[test]
    fn sprint_starts_with_key_or_double_tap_and_stops_when_blocked() {
        let mut sprint = SprintState {
            pressed: true,
            ..SprintState::default()
        };
        assert!(!sprint.update(0.0));
        assert!(sprint.update(1.0));
        sprint.pressed = false;
        assert!(sprint.update(1.0));
        sprint.after_move(true);
        assert!(!sprint.update(1.0));
        sprint.after_move(false);
        assert!(!sprint.update(1.0));
        sprint.pressed = true;
        assert!(sprint.update(1.0));
        assert!(!sprint.update(0.0));

        let mut double_tap = SprintState::default();
        assert!(!double_tap.update(1.0));
        assert!(!double_tap.update(0.0));
        assert!(double_tap.update(1.0));
        assert!(!double_tap.update(-1.0));
        assert!(!double_tap.update(1.0));

        let mut expired = SprintState::default();
        assert!(!expired.update(1.0));
        for _ in 0..7 {
            assert!(!expired.update(0.0));
        }
        assert!(!expired.update(1.0));
    }

    #[test]
    fn glancing_collision_does_not_stop_sprint_but_head_on_collision_does() {
        let state = MotionState {
            position: DVec3::ZERO,
            velocity: DVec3::new(0.1, 0.0, 0.0),
            on_ground: true,
            jump_delay: 0,
        };
        let input = MovementInput {
            forward: 1.0,
            right: 0.0,
            yaw_degrees: 0.0,
            jump: false,
            sprinting: true,
            using_item: false,
            moving_slowly: false,
        };
        let mut glancing = state;
        assert!(!glancing.tick(input, 0.6, |_, requested, _| DVec3::new(
            0.0,
            requested.y,
            requested.z
        )));
        let mut head_on = state;
        assert!(head_on.tick(input, 0.6, |_, requested, _| DVec3::new(
            0.0,
            requested.y,
            0.0
        )));
    }

    #[test]
    fn edge_restraint_does_not_masquerade_as_a_wall_collision() {
        let mut state = MotionState {
            position: DVec3::ZERO,
            velocity: DVec3::new(0.0, 0.0, 0.2),
            on_ground: true,
            jump_delay: 0,
        };
        let input = MovementInput {
            forward: 1.0,
            right: 0.0,
            yaw_degrees: 0.0,
            jump: false,
            sprinting: false,
            using_item: false,
            moving_slowly: true,
        };
        let (collision, actual) = state.tick_resolved(input, 0.6, |_, requested, _| {
            let adjusted = DVec3::new(requested.x, requested.y, 0.0);
            (adjusted, adjusted)
        });
        assert!(!collision);
        assert_eq!(actual.z, 0.0);
        assert!(state.velocity.z > 0.0);
    }
}
