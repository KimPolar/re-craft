//! Java farmland random-tick decisions, with unknown world data kept explicit.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RandomTick {
    Unchanged,
    Moisture(u8),
    Dirt,
}

/// Unknown inputs do not cause speculative drying or conversion to dirt.
pub fn random_tick(
    moisture: u8,
    water: Option<bool>,
    rain: Option<bool>,
    maintained: Option<bool>,
) -> Option<RandomTick> {
    if moisture > 7 {
        return None;
    }
    if water == Some(true) || rain == Some(true) {
        return Some(if moisture < 7 {
            RandomTick::Moisture(7)
        } else {
            RandomTick::Unchanged
        });
    }
    if water != Some(false) || rain != Some(false) {
        return None;
    }
    if moisture > 0 {
        return Some(RandomTick::Moisture(moisture - 1));
    }
    match maintained? {
        true => Some(RandomTick::Unchanged),
        false => Some(RandomTick::Dirt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_weather_or_neighbors_do_not_dry_farmland() {
        assert_eq!(
            random_tick(2, Some(true), None, None),
            Some(RandomTick::Moisture(7))
        );
        assert_eq!(
            random_tick(2, None, Some(true), None),
            Some(RandomTick::Moisture(7))
        );
        assert_eq!(random_tick(2, Some(false), None, Some(false)), None);
        assert_eq!(random_tick(0, Some(false), Some(false), None), None);
        assert_eq!(
            random_tick(0, Some(false), Some(false), Some(false)),
            Some(RandomTick::Dirt)
        );
        assert_eq!(
            random_tick(0, Some(false), Some(false), Some(true)),
            Some(RandomTick::Unchanged)
        );
    }
}
