//! Player movement-event distance accounting from Java 26.3 Entity.move.
use glam::DVec3;

#[derive(Clone, Copy, Debug)]
pub struct Contact {
    pub air: bool,
    pub climbable: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Input {
    pub movement: DVec3,
    pub enabled: bool,
    pub modern: Option<Contact>,
    pub legacy: Option<Contact>,
    pub same_block: bool,
    pub on_ground: bool,
    pub crouching: bool,
    pub on_rails: bool,
    pub swimming: bool,
    pub in_water: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Step,
    Swim,
}

#[derive(Clone, Debug)]
pub struct MovementEvents {
    pub distance: f32,
    pub flying_distance: f32,
    pub next_step: f32,
    pub valid: bool,
}

impl Default for MovementEvents {
    fn default() -> Self {
        Self {
            distance: 0.0,
            flying_distance: 0.0,
            next_step: 1.0,
            valid: true,
        }
    }
}

impl MovementEvents {
    pub fn advance(&mut self, input: Input) -> Option<Event> {
        if !self.valid || !input.enabled {
            return None;
        }
        let Some(modern) = input.modern else {
            self.valid = false;
            return None;
        };
        if !input.movement.is_finite() {
            self.valid = false;
            return None;
        }
        let m = input.movement;
        let scale = f64::from(0.6_f32);
        let full = ((m.x * m.x + m.y * m.y + m.z * m.z).sqrt() * scale) as f32;
        let horizontal = ((m.x * m.x + m.z * m.z).sqrt() * scale) as f32;
        self.distance += if modern.climbable { full } else { horizontal };
        self.flying_distance += full;
        if !self.distance.is_finite() || !self.flying_distance.is_finite() {
            self.valid = false;
            return None;
        }
        if self.distance <= self.next_step || modern.air {
            return None;
        }
        let Some(legacy) = input.legacy else {
            self.valid = false;
            return None;
        };
        let accepts = |contact: Contact| {
            !contact.air
                && (input.on_ground
                    || contact.climbable
                    || (input.crouching && m.y == 0.0)
                    || input.on_rails)
                && !input.swimming
        };
        let old = accepts(legacy);
        let new = !input.same_block && accepts(modern);
        if old || new {
            self.next_step = (self.distance as i32).wrapping_add(1) as f32;
            if (old && input.same_block) || new {
                Some(Event::Step)
            } else {
                None
            }
        } else if input.in_water {
            self.next_step = (self.distance as i32).wrapping_add(1) as f32;
            Some(Event::Swim)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_legacy_contact_only_invalidates_when_a_step_needs_it() {
        let mut tracker = MovementEvents::default();
        let mut input = Input {
            movement: DVec3::X,
            enabled: true,
            modern: Some(Contact {
                air: false,
                climbable: false,
            }),
            legacy: None,
            same_block: false,
            on_ground: true,
            crouching: false,
            on_rails: false,
            swimming: false,
            in_water: false,
        };
        assert_eq!(tracker.advance(input), None);
        assert!(tracker.valid);
        input.modern = Some(Contact {
            air: true,
            climbable: false,
        });
        assert_eq!(tracker.advance(input), None);
        assert!(tracker.valid);
        input.modern = Some(Contact {
            air: false,
            climbable: false,
        });
        assert_eq!(tracker.advance(input), None);
        assert!(!tracker.valid);
    }
}
