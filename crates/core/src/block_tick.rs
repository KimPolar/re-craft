//! IDs of block ticks whose simulation and persistence share the same rules.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AirKind {
    Air,
    Cave,
    Void,
}

impl AirKind {
    pub const ALL: [Self; 3] = [Self::Air, Self::Cave, Self::Void];

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "minecraft:air" | "air" => Some(Self::Air),
            "minecraft:cave_air" | "cave_air" => Some(Self::Cave),
            "minecraft:void_air" | "void_air" => Some(Self::Void),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Air => "minecraft:air",
            Self::Cave => "minecraft:cave_air",
            Self::Void => "minecraft:void_air",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SpeleothemKind {
    Dripstone,
    Sulfur,
}

impl SpeleothemKind {
    pub const ALL: [Self; 2] = [Self::Dripstone, Self::Sulfur];

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "minecraft:pointed_dripstone" => Some(Self::Dripstone),
            "minecraft:sulfur_spike" => Some(Self::Sulfur),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Dripstone => "minecraft:pointed_dripstone",
            Self::Sulfur => "minecraft:sulfur_spike",
        }
    }
}
