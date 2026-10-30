//! Client right-click repeat timing for supported interactions.

#[derive(Default)]
pub(crate) struct UseRepeat {
    held: bool,
    cooldown: u8,
}

impl UseRepeat {
    pub fn advance_tick(&mut self) {
        self.cooldown = self.cooldown.saturating_sub(1);
    }
    pub fn set_held(&mut self, held: bool) {
        self.held = held;
    }
    pub fn release(&mut self) {
        self.held = false;
    }
    pub fn used(&mut self) {
        self.cooldown = 4;
    }
    pub fn ready(&self) -> bool {
        self.held && self.cooldown == 0
    }
}
