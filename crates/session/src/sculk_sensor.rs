//! State rules for Java 26.3 sculk sensors. World event dispatch, neighbor
//! notifications, block entity persistence and scheduled ticks are separate.

use glam::IVec3;
use native_world::loaded::BlockState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Inactive,
    Active,
    Cooldown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sensor {
    pub phase: Phase,
    pub power: u8,
    pub waterlogged: bool,
    /// Facing is present only for a calibrated sensor.
    pub facing: Option<IVec3>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition {
    pub state: Sensor,
    /// A new block tick delay, if the transition schedules one.
    pub tick_after: Option<u8>,
    /// Notify neighbors at the sensor and at the block below it.
    pub notify_neighbors: bool,
    pub emit_clicking_event: bool,
    pub play_click: bool,
    pub play_stop: bool,
    pub last_frequency: Option<u8>,
}

impl Sensor {
    /// Reject incomplete, invalid, and extra state properties.
    pub fn from_block(state: &BlockState) -> Option<Self> {
        let calibrated = match state.name.as_str() {
            "minecraft:sculk_sensor" => false,
            "minecraft:calibrated_sculk_sensor" => true,
            _ => return None,
        };
        if state.properties.len() != if calibrated { 4 } else { 3 } {
            return None;
        }
        let phase = match state.properties.get("sculk_sensor_phase")?.as_str() {
            "inactive" => Phase::Inactive,
            "active" => Phase::Active,
            "cooldown" => Phase::Cooldown,
            _ => return None,
        };
        let value = state.properties.get("power")?;
        let power = value.parse::<u8>().ok()?;
        if power > 15 || power.to_string() != *value {
            return None;
        }
        let waterlogged = match state.properties.get("waterlogged")?.as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        };
        let facing = if calibrated {
            Some(match state.properties.get("facing")?.as_str() {
                "north" => IVec3::NEG_Z,
                "south" => IVec3::Z,
                "west" => IVec3::NEG_X,
                "east" => IVec3::X,
                _ => return None,
            })
        } else {
            None
        };
        Some(Self {
            phase,
            power,
            waterlogged,
            facing,
        })
    }

    /// Produce a canonical block state without dropping any sensor property.
    pub fn to_block(self) -> Option<BlockState> {
        if self.power > 15 {
            return None;
        }
        let facing = match self.facing {
            None => None,
            Some(IVec3::NEG_Z) => Some("north"),
            Some(IVec3::Z) => Some("south"),
            Some(IVec3::NEG_X) => Some("west"),
            Some(IVec3::X) => Some("east"),
            Some(_) => return None,
        };
        let mut properties = std::collections::BTreeMap::from([
            (
                "sculk_sensor_phase".to_owned(),
                match self.phase {
                    Phase::Inactive => "inactive",
                    Phase::Active => "active",
                    Phase::Cooldown => "cooldown",
                }
                .to_owned(),
            ),
            ("power".to_owned(), self.power.to_string()),
            ("waterlogged".to_owned(), self.waterlogged.to_string()),
        ]);
        if let Some(facing) = facing {
            properties.insert("facing".into(), facing.into());
        }
        Some(BlockState {
            name: if facing.is_some() {
                "minecraft:calibrated_sculk_sensor"
            } else {
                "minecraft:sculk_sensor"
            }
            .into(),
            properties,
        })
    }

    pub fn radius(self) -> i32 {
        if self.facing.is_some() { 16 } else { 8 }
    }
    pub fn active_ticks(self) -> u8 {
        if self.facing.is_some() { 10 } else { 30 }
    }

    /// Direction of the block queried for a calibrated sensor's input.
    pub fn back_face(self) -> Option<IVec3> {
        self.facing.map(|facing| -facing)
    }

    /// Input strength is only queried for calibrated sensors, at the back face.
    /// Unknown input must be represented as None rather than assumed to be zero.
    pub fn can_receive(
        self,
        event: &str,
        same_block: bool,
        back_signal: Option<u8>,
    ) -> Option<bool> {
        let frequency = crate::vibration::frequency(event);
        if self.facing.is_some() {
            let signal = back_signal?;
            if signal > 15 {
                return None;
            }
            if signal != 0 && signal != frequency {
                return Some(false);
            }
        }
        let own_change = same_block
            && matches!(
                event,
                "minecraft:block_place"
                    | "minecraft:block_destroy"
                    | "block_place"
                    | "block_destroy"
            );
        Some(!own_change && frequency != 0 && self.phase == Phase::Inactive)
    }

    /// Weak and direct signal in Java's query direction.
    pub fn signal(self, direction: IVec3) -> Option<(u8, u8)> {
        if ![
            IVec3::X,
            IVec3::NEG_X,
            IVec3::Y,
            IVec3::NEG_Y,
            IVec3::Z,
            IVec3::NEG_Z,
        ]
        .contains(&direction)
        {
            return None;
        }
        let weak = if self.facing == Some(direction) {
            0
        } else {
            self.power
        };
        Some((weak, if direction == IVec3::Y { weak } else { 0 }))
    }

    /// Delivery rechecks phase but does not recheck the calibrated input.
    pub fn arrive(self, event: &str, block_distance: f32) -> Option<Transition> {
        if self.phase != Phase::Inactive {
            return None;
        }
        let power = crate::vibration::redstone_strength(block_distance, self.radius())?;
        let power = u8::try_from(power).ok().filter(|p| *p <= 15)?;
        let mut state = self;
        state.phase = Phase::Active;
        state.power = power;
        Some(Transition {
            state,
            tick_after: Some(self.active_ticks()),
            notify_neighbors: true,
            emit_clicking_event: true,
            play_click: !self.waterlogged,
            play_stop: false,
            last_frequency: Some(crate::vibration::frequency(event)),
        })
    }

    pub fn scheduled_tick(self) -> Option<Transition> {
        let mut state = self;
        let (tick_after, notify_neighbors, play_stop) = match self.phase {
            Phase::Inactive => return None,
            Phase::Active => {
                state.phase = Phase::Cooldown;
                state.power = 0;
                (Some(10), true, false)
            }
            Phase::Cooldown => {
                state.phase = Phase::Inactive;
                (None, false, !self.waterlogged)
            }
        };
        Some(Transition {
            state,
            tick_after,
            notify_neighbors,
            emit_clicking_event: false,
            play_click: false,
            play_stop,
            last_frequency: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivery_and_cooldown_are_separate_from_initial_filter() {
        let sensor = Sensor {
            phase: Phase::Inactive,
            power: 0,
            waterlogged: false,
            facing: Some(IVec3::NEG_Z),
        };
        assert_eq!(
            sensor.can_receive("minecraft:splash", false, Some(2)),
            Some(true)
        );
        assert_eq!(sensor.back_face(), Some(IVec3::Z));
        assert_eq!(sensor.can_receive("minecraft:splash", false, None), None);
        assert_eq!(
            sensor.can_receive("minecraft:block_place", true, Some(0)),
            Some(false)
        );
        let active = sensor.arrive("minecraft:splash", 4.0).unwrap();
        assert_eq!(
            (active.state.power, active.tick_after, active.last_frequency),
            (12, Some(10), Some(2))
        );
        assert!(active.play_click && active.emit_clicking_event);
        assert_eq!(
            active.state.can_receive("minecraft:splash", false, Some(2)),
            Some(false)
        );
        let cooldown = active.state.scheduled_tick().unwrap();
        assert_eq!(
            (
                cooldown.state.phase,
                cooldown.state.power,
                cooldown.tick_after
            ),
            (Phase::Cooldown, 0, Some(10))
        );
        let inactive = cooldown.state.scheduled_tick().unwrap();
        assert!(inactive.play_stop);
        assert_eq!(inactive.state.phase, Phase::Inactive);
        assert!(inactive.state.scheduled_tick().is_none());
    }

    #[test]
    fn block_states_round_trip_and_invalid_values_are_rejected() {
        for phase in [Phase::Inactive, Phase::Active, Phase::Cooldown] {
            for facing in [
                None,
                Some(IVec3::NEG_Z),
                Some(IVec3::Z),
                Some(IVec3::NEG_X),
                Some(IVec3::X),
            ] {
                for power in 0..=15 {
                    for waterlogged in [false, true] {
                        let state = Sensor {
                            phase,
                            facing,
                            power,
                            waterlogged,
                        };
                        let block = state.to_block().unwrap();
                        assert_eq!(Sensor::from_block(&block), Some(state));
                    }
                }
            }
        }
        let invalid = Sensor {
            phase: Phase::Inactive,
            power: 16,
            waterlogged: false,
            facing: None,
        };
        assert!(invalid.to_block().is_none());
        let invalid = Sensor {
            power: 0,
            facing: Some(IVec3::Y),
            ..invalid
        };
        assert!(invalid.to_block().is_none());
        let mut extra = Sensor {
            facing: None,
            ..invalid
        }
        .to_block()
        .unwrap();
        extra.properties.insert("unexpected".into(), "true".into());
        assert!(Sensor::from_block(&extra).is_none());
    }
}
