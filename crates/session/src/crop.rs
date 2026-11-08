//! Java crop random-tick growth and its shared level-random consumption.

use native_core::random::LegacyRandom;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Soil {
    Other,
    Dry,
    Wet,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crop {
    Wheat,
    Carrots,
    Potatoes,
    Beetroots,
}

impl Crop {
    pub fn max_age(self) -> u8 {
        if self == Self::Beetroots { 3 } else { 7 }
    }
}

/// A crop survives only with enough raw light and a tagged supporting block.
pub fn can_survive(brightness: Option<u8>, supported: Option<bool>) -> Option<bool> {
    if brightness.is_some_and(|value| value < 8) || supported == Some(false) {
        return Some(false);
    }
    match (brightness, supported) {
        (Some(_), Some(true)) => Some(true),
        _ => None,
    }
}

/// Apply the crop's bonemeal increase, or return None for a mature crop.
pub fn bonemeal(crop: Crop, age: u8, random: &mut LegacyRandom) -> Option<u8> {
    if age >= crop.max_age() {
        return None;
    }
    let roll = random.next_int_bound(4).expect("positive bound") as u8 + 2;
    let increase = if crop == Crop::Beetroots {
        roll / 3
    } else {
        roll
    };
    Some(age.saturating_add(increase).min(crop.max_age()))
}

/// Soil is ordered by x then z from -1 to 1. Neighbors are north, south,
/// west, east, northwest, northeast, southeast and southwest.
pub fn growth_speed(soil: [Soil; 9], neighbors: [bool; 8]) -> f32 {
    let mut speed = 1.0_f32;
    for (i, soil) in soil.into_iter().enumerate() {
        let mut contribution = match soil {
            Soil::Other => 0.0_f32,
            Soil::Dry => 1.0_f32,
            Soil::Wet => 3.0_f32,
        };
        if i != 4 {
            contribution /= 4.0_f32;
        }
        speed += contribution;
    }
    if ((neighbors[0] || neighbors[1]) && (neighbors[2] || neighbors[3]))
        || neighbors[4..].iter().any(|&same| same)
    {
        speed /= 2.0_f32;
    }
    speed
}

/// Returns the resulting age. This models a dispatched random tick, including
/// the beetroot gate that precedes its brightness and age checks.
pub fn random_tick(
    crop: Crop,
    age: u8,
    brightness: u8,
    soil: [Soil; 9],
    neighbors: [bool; 8],
    random: &mut LegacyRandom,
) -> u8 {
    random_tick_checked(
        crop,
        age,
        Some(brightness),
        || Some(growth_speed(soil, neighbors)),
        random,
    )
    .expect("complete crop inputs")
}

/// Returns `None` if a dispatched tick needs unavailable world data.
/// Brightness and growth inputs are only consulted after the beetroot gate.
pub fn random_tick_checked(
    crop: Crop,
    age: u8,
    brightness: Option<u8>,
    growth: impl FnOnce() -> Option<f32>,
    random: &mut LegacyRandom,
) -> Option<u8> {
    if crop == Crop::Beetroots && random.next_int_bound(3).expect("positive bound") == 0 {
        return Some(age);
    }
    if brightness? >= 9 && age < crop.max_age() {
        let bound = (25.0_f32 / growth()?) as i32 + 1;
        if random.next_int_bound(bound).expect("positive bound") == 0 {
            return Some(age + 1);
        }
    }
    Some(age)
}
