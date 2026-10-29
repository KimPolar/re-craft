//! The ordinary plank blocks supported by the in-memory play sessions.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Planks {
    Oak,
    Spruce,
    Birch,
    Jungle,
    Acacia,
    DarkOak,
    Mangrove,
    Cherry,
    PaleOak,
    Bamboo,
    Crimson,
    Warped,
    Poplar,
}

impl Planks {
    pub const ALL: [Self; 13] = [
        Self::Oak,
        Self::Spruce,
        Self::Birch,
        Self::Jungle,
        Self::Acacia,
        Self::DarkOak,
        Self::Mangrove,
        Self::Cherry,
        Self::PaleOak,
        Self::Bamboo,
        Self::Crimson,
        Self::Warped,
        Self::Poplar,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Oak => "minecraft:oak_planks",
            Self::Spruce => "minecraft:spruce_planks",
            Self::Birch => "minecraft:birch_planks",
            Self::Jungle => "minecraft:jungle_planks",
            Self::Acacia => "minecraft:acacia_planks",
            Self::DarkOak => "minecraft:dark_oak_planks",
            Self::Mangrove => "minecraft:mangrove_planks",
            Self::Cherry => "minecraft:cherry_planks",
            Self::PaleOak => "minecraft:pale_oak_planks",
            Self::Bamboo => "minecraft:bamboo_planks",
            Self::Crimson => "minecraft:crimson_planks",
            Self::Warped => "minecraft:warped_planks",
            Self::Poplar => "minecraft:poplar_planks",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|planks| planks.id() == id)
    }

    /// Position in `ALL`, used by the fixed demo atlas.
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// A vanilla log, wood, stem, hyphae or bamboo block item.
/// The axis is a block-state property and is stored separately by the world.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Wood {
    family: Planks,
    stripped: bool,
    bark: bool,
}

const WOOD_IDS: [[&str; 4]; 13] = [
    [
        "minecraft:oak_log",
        "minecraft:oak_wood",
        "minecraft:stripped_oak_log",
        "minecraft:stripped_oak_wood",
    ],
    [
        "minecraft:spruce_log",
        "minecraft:spruce_wood",
        "minecraft:stripped_spruce_log",
        "minecraft:stripped_spruce_wood",
    ],
    [
        "minecraft:birch_log",
        "minecraft:birch_wood",
        "minecraft:stripped_birch_log",
        "minecraft:stripped_birch_wood",
    ],
    [
        "minecraft:jungle_log",
        "minecraft:jungle_wood",
        "minecraft:stripped_jungle_log",
        "minecraft:stripped_jungle_wood",
    ],
    [
        "minecraft:acacia_log",
        "minecraft:acacia_wood",
        "minecraft:stripped_acacia_log",
        "minecraft:stripped_acacia_wood",
    ],
    [
        "minecraft:dark_oak_log",
        "minecraft:dark_oak_wood",
        "minecraft:stripped_dark_oak_log",
        "minecraft:stripped_dark_oak_wood",
    ],
    [
        "minecraft:mangrove_log",
        "minecraft:mangrove_wood",
        "minecraft:stripped_mangrove_log",
        "minecraft:stripped_mangrove_wood",
    ],
    [
        "minecraft:cherry_log",
        "minecraft:cherry_wood",
        "minecraft:stripped_cherry_log",
        "minecraft:stripped_cherry_wood",
    ],
    [
        "minecraft:pale_oak_log",
        "minecraft:pale_oak_wood",
        "minecraft:stripped_pale_oak_log",
        "minecraft:stripped_pale_oak_wood",
    ],
    [
        "minecraft:bamboo_block",
        "",
        "minecraft:stripped_bamboo_block",
        "",
    ],
    [
        "minecraft:crimson_stem",
        "minecraft:crimson_hyphae",
        "minecraft:stripped_crimson_stem",
        "minecraft:stripped_crimson_hyphae",
    ],
    [
        "minecraft:warped_stem",
        "minecraft:warped_hyphae",
        "minecraft:stripped_warped_stem",
        "minecraft:stripped_warped_hyphae",
    ],
    [
        "minecraft:poplar_log",
        "minecraft:poplar_wood",
        "minecraft:stripped_poplar_log",
        "minecraft:stripped_poplar_wood",
    ],
];

const WOOD_TEXTURES: [&str; 13] = [
    "oak_log",
    "spruce_log",
    "birch_log",
    "jungle_log",
    "acacia_log",
    "dark_oak_log",
    "mangrove_log",
    "cherry_log",
    "pale_oak_log",
    "bamboo_block",
    "crimson_stem",
    "warped_stem",
    "poplar_log",
];

impl Wood {
    /// Number of distinct side/end textures, including stripped variants.
    pub const TEXTURE_COUNT: u32 = Planks::ALL.len() as u32 * 4;

    pub fn all() -> impl Iterator<Item = Self> {
        Planks::ALL.into_iter().flat_map(|family| {
            [false, true].into_iter().flat_map(move |stripped| {
                [false, true].into_iter().filter_map(move |bark| {
                    (family != Planks::Bamboo || !bark).then_some(Self {
                        family,
                        stripped,
                        bark,
                    })
                })
            })
        })
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::all().find(|wood| wood.id() == id)
    }

    pub fn id(self) -> &'static str {
        WOOD_IDS[self.family.index()][usize::from(self.stripped) * 2 + usize::from(self.bark)]
    }

    /// Index in the sequence returned by `texture_names`, before the side/end offset.
    pub fn texture_index(self) -> u32 {
        self.family.index() as u32 * 4 + u32::from(self.stripped) * 2
    }

    pub fn bark_all_sides(self) -> bool {
        self.bark
    }

    /// The vanilla cherry and bamboo log models have axis-specific locked UVs.
    pub fn uses_locked_model(self) -> bool {
        !self.bark && matches!(self.family, Planks::Cherry | Planks::Bamboo)
    }

    /// Ordinary logs use the horizontal-column model when placed on their side.
    pub fn uses_horizontal_model(self) -> bool {
        !self.bark
            && !matches!(
                self.family,
                Planks::Cherry | Planks::Bamboo | Planks::Crimson | Planks::Warped
            )
    }

    pub fn texture_names() -> impl Iterator<Item = String> {
        WOOD_TEXTURES.into_iter().flat_map(|name| {
            [false, true].into_iter().flat_map(move |stripped| {
                let base = if stripped {
                    format!("stripped_{name}")
                } else {
                    name.to_owned()
                };
                [format!("{base}.png"), format!("{base}_top.png")]
            })
        })
    }
}
