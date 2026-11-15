//! The vanilla leaf block IDs whose scheduled ticks are simulated.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LeafKind(&'static str);

impl LeafKind {
    pub const ALL: [&'static str; 14] = [
        "minecraft:oak_leaves",
        "minecraft:spruce_leaves",
        "minecraft:birch_leaves",
        "minecraft:jungle_leaves",
        "minecraft:acacia_leaves",
        "minecraft:dark_oak_leaves",
        "minecraft:mangrove_leaves",
        "minecraft:cherry_leaves",
        "minecraft:pale_oak_leaves",
        "minecraft:red_poplar_leaves",
        "minecraft:orange_poplar_leaves",
        "minecraft:yellow_poplar_leaves",
        "minecraft:azalea_leaves",
        "minecraft:flowering_azalea_leaves",
    ];

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|name| *name == id || name.strip_prefix("minecraft:") == Some(id))
            .map(Self)
    }

    pub fn id(self) -> &'static str {
        self.0
    }
}
