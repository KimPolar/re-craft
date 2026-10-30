//! Vanilla ore block identities shared by placement and demo rendering.

macro_rules! ores {
    ($( $variant:ident => $name:literal ),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u8)]
        pub enum Ore { $( $variant ),+ }

        impl Ore {
            pub const ALL: [Self; 18] = [$( Self::$variant ),+];
            pub const fn id(self) -> &'static str {
                match self { $( Self::$variant => concat!("minecraft:", $name) ),+ }
            }
            pub fn from_id(id: &str) -> Option<Self> {
                match id { $( concat!("minecraft:", $name) => Some(Self::$variant), )+ _ => None }
            }
            pub const fn index(self) -> u32 { self as u32 }
            pub const fn has_lit_state(self) -> bool {
                matches!(self, Self::Redstone | Self::DeepslateRedstone)
            }
            pub fn texture_name(self) -> String {
                format!("{}.png", self.id().strip_prefix("minecraft:").unwrap())
            }
        }
    }
}

ores! {
    Coal => "coal_ore", DeepslateCoal => "deepslate_coal_ore",
    Iron => "iron_ore", DeepslateIron => "deepslate_iron_ore",
    Copper => "copper_ore", DeepslateCopper => "deepslate_copper_ore",
    Gold => "gold_ore", DeepslateGold => "deepslate_gold_ore",
    Diamond => "diamond_ore", DeepslateDiamond => "deepslate_diamond_ore",
    Emerald => "emerald_ore", DeepslateEmerald => "deepslate_emerald_ore",
    Lapis => "lapis_ore", DeepslateLapis => "deepslate_lapis_ore",
    Redstone => "redstone_ore", DeepslateRedstone => "deepslate_redstone_ore",
    NetherGold => "nether_gold_ore", NetherQuartz => "nether_quartz_ore",
}
