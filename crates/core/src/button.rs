//! Vanilla button identities and their 26.3 interaction properties.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Button(&'static str);

impl Button {
    pub const ALL: [Self; 15] = [
        Self("minecraft:stone_button"),
        Self("minecraft:polished_blackstone_button"),
        Self("minecraft:oak_button"),
        Self("minecraft:spruce_button"),
        Self("minecraft:birch_button"),
        Self("minecraft:jungle_button"),
        Self("minecraft:acacia_button"),
        Self("minecraft:dark_oak_button"),
        Self("minecraft:pale_oak_button"),
        Self("minecraft:mangrove_button"),
        Self("minecraft:cherry_button"),
        Self("minecraft:bamboo_button"),
        Self("minecraft:crimson_button"),
        Self("minecraft:warped_button"),
        Self("minecraft:poplar_button"),
    ];

    pub fn from_id(id: &str) -> Option<Self> {
        if !id.ends_with("_button") {
            return None;
        }
        Self::ALL
            .into_iter()
            .find(|button| button.0 == id || button.0.strip_prefix("minecraft:") == Some(id))
    }

    pub const fn id(self) -> &'static str {
        self.0
    }

    pub fn duration(self) -> u32 {
        if matches!(
            self.0,
            "minecraft:stone_button" | "minecraft:polished_blackstone_button"
        ) {
            20
        } else {
            30
        }
    }

    pub fn arrow_sensitive(self) -> bool {
        self.duration() == 30
    }

    pub fn sound(self, powered: bool) -> &'static str {
        match (self.0, powered) {
            ("minecraft:stone_button" | "minecraft:polished_blackstone_button", true) => {
                "block.stone_button.click_on"
            }
            ("minecraft:stone_button" | "minecraft:polished_blackstone_button", false) => {
                "block.stone_button.click_off"
            }
            ("minecraft:bamboo_button", true) => "block.bamboo_wood_button.click_on",
            ("minecraft:bamboo_button", false) => "block.bamboo_wood_button.click_off",
            ("minecraft:cherry_button", true) => "block.cherry_wood_button.click_on",
            ("minecraft:cherry_button", false) => "block.cherry_wood_button.click_off",
            ("minecraft:crimson_button" | "minecraft:warped_button", true) => {
                "block.nether_wood_button.click_on"
            }
            ("minecraft:crimson_button" | "minecraft:warped_button", false) => {
                "block.nether_wood_button.click_off"
            }
            (_, true) => "block.wooden_button.click_on",
            (_, false) => "block.wooden_button.click_off",
        }
    }
}
