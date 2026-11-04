//! In-memory 26.3 `level.dat` data for a newly created world.
//!
//! This draft does not initialize the other saved-data files, player, chunks,
//! entities or data packs, and must not by itself be treated as a playable world.
use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use native_resources::{datapacks::DataPackSettings, packs::ResourceId};

use crate::{
    nbt::{JavaString, NamedTag, Tag, TagType},
    world::{DimensionId, RespawnData, TARGET_DATA_VERSION},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewWorldGameType {
    Survival,
    Creative,
    Adventure,
    Spectator,
}
impl NewWorldGameType {
    fn id(self) -> i32 {
        match self {
            Self::Survival => 0,
            Self::Creative => 1,
            Self::Adventure => 2,
            Self::Spectator => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NewWorldDifficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}
impl NewWorldDifficulty {
    fn id(self) -> &'static str {
        match self {
            Self::Peaceful => "peaceful",
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Hard => "hard",
        }
    }
}

/// Values must be chosen by the world-creation session from one pack stack.
/// `None` for packs means Java's default configuration.
pub struct NewLevelData<'a> {
    pub name: &'a str,
    pub game_type: NewWorldGameType,
    pub difficulty: NewWorldDifficulty,
    pub hardcore: bool,
    pub difficulty_locked: bool,
    pub allow_commands: bool,
    pub spawn: &'a RespawnData,
    pub game_time: i64,
    pub initialized: bool,
    pub last_played_ms: i64,
    pub singleplayer_uuid: Option<[i32; 4]>,
    pub packs: Option<&'a DataPackSettings>,
    pub server_brands: &'a [String],
    pub was_modded: bool,
}

fn string_list(values: impl IntoIterator<Item = String>) -> Tag {
    let values: Vec<_> = values
        .into_iter()
        .map(|value| Tag::String(JavaString::from(value.as_str())))
        .collect();
    Tag::List {
        element_type: if values.is_empty() {
            TagType::End
        } else {
            TagType::String
        },
        values,
    }
}

/// Construct the root document without performing filesystem I/O. The caller
/// must validate the active data packs and complete every required world file.
pub fn draft_new_level_data(input: &NewLevelData<'_>) -> Result<NamedTag> {
    let dimension = format!(
        "{}:{}",
        input.spawn.dimension.namespace, input.spawn.dimension.path
    );
    DimensionId::parse(&dimension)?;
    ensure!(
        input.spawn.yaw.is_finite() && (-180.0..=180.0).contains(&input.spawn.yaw),
        "invalid spawn yaw"
    );
    ensure!(
        input.spawn.pitch.is_finite() && (-90.0..=90.0).contains(&input.spawn.pitch),
        "invalid spawn pitch"
    );
    let mut brands = BTreeSet::new();
    let brands = input
        .server_brands
        .iter()
        .filter(|brand| brands.insert((*brand).clone()))
        .cloned();
    let mut fields = vec![
        ("ServerBrands".into(), string_list(brands)),
        ("WasModded".into(), Tag::Byte(i8::from(input.was_modded))),
        (
            "Version".into(),
            Tag::Compound(vec![
                ("Name".into(), Tag::String("26.3".into())),
                ("Id".into(), Tag::Int(TARGET_DATA_VERSION)),
                ("Snapshot".into(), Tag::Byte(0)),
                ("Series".into(), Tag::String("main".into())),
            ]),
        ),
        (
            "version_history".into(),
            Tag::List {
                element_type: TagType::Int,
                values: vec![Tag::Int(TARGET_DATA_VERSION)],
            },
        ),
        ("DataVersion".into(), Tag::Int(TARGET_DATA_VERSION)),
        ("GameType".into(), Tag::Int(input.game_type.id())),
        (
            "spawn".into(),
            Tag::Compound(vec![
                (
                    "dimension".into(),
                    Tag::String(JavaString::from(dimension.as_str())),
                ),
                ("pos".into(), Tag::IntArray(input.spawn.position.to_vec())),
                ("yaw".into(), Tag::Float(input.spawn.yaw)),
                ("pitch".into(), Tag::Float(input.spawn.pitch)),
            ]),
        ),
        ("Time".into(), Tag::Long(input.game_time)),
        ("LastPlayed".into(), Tag::Long(input.last_played_ms)),
        (
            "LevelName".into(),
            Tag::String(JavaString::from(input.name.trim_matches(|c| c <= '\u{20}'))),
        ),
        ("version".into(), Tag::Int(19133)),
        (
            "allowCommands".into(),
            Tag::Byte(i8::from(input.allow_commands)),
        ),
        ("initialized".into(), Tag::Byte(i8::from(input.initialized))),
        (
            "difficulty_settings".into(),
            Tag::Compound(vec![
                (
                    "difficulty".into(),
                    Tag::String(input.difficulty.id().into()),
                ),
                ("hardcore".into(), Tag::Byte(i8::from(input.hardcore))),
                (
                    "locked".into(),
                    Tag::Byte(i8::from(input.difficulty_locked)),
                ),
            ]),
        ),
    ];
    if let Some(uuid) = input.singleplayer_uuid {
        fields.push(("singleplayer_uuid".into(), Tag::IntArray(uuid.to_vec())));
    }
    if let Some(packs) = input.packs {
        if packs.enabled != ["vanilla"] || !packs.disabled.is_empty() {
            fields.push((
                "DataPacks".into(),
                Tag::Compound(vec![
                    ("Enabled".into(), string_list(packs.enabled.iter().cloned())),
                    (
                        "Disabled".into(),
                        string_list(packs.disabled.iter().cloned()),
                    ),
                ]),
            ));
        }
        let mut features = BTreeSet::new();
        for name in &packs.enabled_features {
            let id = ResourceId::parse(name)?;
            let id = format!("{}:{}", id.namespace, id.path);
            ensure!(
                matches!(
                    id.as_str(),
                    "minecraft:vanilla"
                        | "minecraft:trade_rebalance"
                        | "minecraft:redstone_experiments"
                        | "minecraft:minecart_improvements"
                ),
                "unknown enabled feature {id}"
            );
            features.insert(id);
        }
        if features != BTreeSet::from(["minecraft:vanilla".to_owned()]) {
            fields.push(("enabled_features".into(), string_list(features)));
        }
    }
    Ok(NamedTag {
        name: "".into(),
        value: Tag::Compound(vec![("Data".into(), Tag::Compound(fields))]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(spawn: &RespawnData) -> NewLevelData<'_> {
        NewLevelData {
            name: "  World  ",
            game_type: NewWorldGameType::Survival,
            difficulty: NewWorldDifficulty::Normal,
            hardcore: false,
            difficulty_locked: false,
            allow_commands: false,
            spawn,
            game_time: 0,
            initialized: false,
            last_played_ms: 42,
            singleplayer_uuid: None,
            packs: None,
            server_brands: &[],
            was_modded: false,
        }
    }

    #[test]
    fn creates_default_level_data_and_round_trips_nbt() {
        let spawn = RespawnData::default();
        let document = draft_new_level_data(&draft(&spawn)).unwrap();
        let data = document.value.get("Data").unwrap();
        assert_eq!(data.get("LevelName"), Some(&Tag::String("World".into())));
        assert_eq!(data.get("DataVersion"), Some(&Tag::Int(5023)));
        assert!(data.get("DataPacks").is_none());
        assert_eq!(
            crate::nbt::read(&crate::nbt::write(&document).unwrap()).unwrap(),
            document
        );
    }

    #[test]
    fn rejects_invalid_angles_and_unknown_features() {
        let spawn = RespawnData {
            yaw: f32::NAN,
            ..RespawnData::default()
        };
        assert!(draft_new_level_data(&draft(&spawn)).is_err());
        let spawn = RespawnData::default();
        let packs = DataPackSettings {
            enabled: vec!["vanilla".into()],
            disabled: vec![],
            enabled_features: vec!["example:unknown".into()],
        };
        let mut input = draft(&spawn);
        input.packs = Some(&packs);
        assert!(draft_new_level_data(&input).is_err());
    }
}
