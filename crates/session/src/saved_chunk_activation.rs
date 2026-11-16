//! Checks for saved preparation work that the live session cannot yet run.
//! Passing this check does not establish Java chunk-holder or ticket readiness.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_persistence::nbt::{self, Tag, TagType};
use native_world::loaded::{
    ChunkPos, SavedBlockEntities, SavedBlockTick, SavedChunkActivation, SavedFluidTick,
};

use crate::loaded::BuildBounds;

pub(crate) struct SavedTickInputs<'a> {
    pub position: ChunkPos,
    pub blocks: &'a [SavedBlockTick],
    pub fluids: &'a [SavedFluidTick],
    pub supports_block: fn(&str) -> bool,
}

/// Brush counts are not saved by Java and start at zero after loading. A saved
/// hit direction is cleared by the first tick, so leave it unhandled until
/// the live block-entity state can be updated together with the chunk.
pub(crate) fn brushable_reset_is_inert(entities: &SavedBlockEntities, position: [i32; 3]) -> bool {
    let bytes = match entities {
        SavedBlockEntities::Empty => return true,
        SavedBlockEntities::Unavailable => return false,
        SavedBlockEntities::Encoded(bytes) => bytes,
    };
    let Ok(document) = nbt::read(bytes) else {
        return false;
    };
    let Tag::List {
        element_type,
        values,
    } = document.value
    else {
        return false;
    };
    if values.is_empty() {
        return matches!(element_type, TagType::End | TagType::Compound);
    }
    if element_type != TagType::Compound {
        return false;
    }
    let mut found = false;
    for entity in values {
        let Tag::Compound(fields) = &entity else {
            return false;
        };
        let mut names = BTreeSet::new();
        if fields.iter().any(|(name, _)| !names.insert(name.0.clone())) {
            return false;
        }
        let [Some(Tag::Int(x)), Some(Tag::Int(y)), Some(Tag::Int(z))] =
            [entity.get("x"), entity.get("y"), entity.get("z")]
        else {
            return false;
        };
        if [*x, *y, *z] != position {
            continue;
        }
        if found {
            return false;
        }
        found = true;
        if !matches!(entity.get("id"), Some(Tag::String(id)) if id.matches("minecraft:brushable_block") || id.matches("brushable_block"))
            || entity.get("hit_direction").is_some()
            || !matches!(entity.get("keepPacked"), None | Some(Tag::Byte(0)))
        {
            return false;
        }
    }
    true
}

pub(crate) fn check_saved_work(
    saved: &SavedChunkActivation,
    entities: &SavedBlockEntities,
    bounds: BuildBounds,
    ticks: SavedTickInputs<'_>,
) -> Result<()> {
    read_saved_work(saved, entities, bounds, ticks, false).map(|_| ())
}

/// Retain the original ordered marks when a caller will run the complete
/// isolated pass. This does not establish chunk-holder or save readiness.
pub(crate) fn read_saved_work(
    saved: &SavedChunkActivation,
    entities: &SavedBlockEntities,
    bounds: BuildBounds,
    ticks: SavedTickInputs<'_>,
    allow_postprocessing: bool,
) -> Result<native_core::postprocessing::PostProcessing> {
    let height = bounds
        .max_y
        .checked_sub(bounds.min_y)
        .context("invalid dimension height")?;
    let mut marks = native_core::postprocessing::PostProcessing::new(bounds.min_y, height)
        .context("invalid postprocessing bounds")?;
    let mut block_ticks_seen = false;
    let mut fluid_ticks_seen = false;
    match saved {
        SavedChunkActivation::Empty => {}
        SavedChunkActivation::Unavailable => bail!("chunk preparation data is unavailable"),
        SavedChunkActivation::Encoded(bytes) => {
            let document = nbt::read(bytes).context("cannot read chunk preparation data")?;
            let Tag::Compound(fields) = document.value else {
                bail!("chunk preparation data is not a compound");
            };
            let mut seen = BTreeSet::new();
            for (name, value) in fields {
                ensure!(seen.insert(name.0.clone()), "duplicate preparation field");
                if name.matches("PostProcessing") {
                    if !matches!(&value, Tag::List { element_type: TagType::End | TagType::List, values } if values.is_empty())
                    {
                        marks = native_persistence::postprocessing::decode(
                            &value,
                            bounds.min_y,
                            height,
                        )?;
                    }
                    if !allow_postprocessing {
                        ensure!(
                            marks.sections().iter().all(Vec::is_empty),
                            "saved postprocessing needs block and fluid updates"
                        );
                    }
                } else if name.matches("UpgradeData") {
                    check_upgrade_data(&value, bounds)?;
                } else if name.matches("block_ticks") {
                    block_ticks_seen = true;
                    check_tick_list(
                        &value,
                        ticks.position,
                        ticks.blocks.iter().map(|tick| {
                            (
                                tick.block.as_str(),
                                tick.position,
                                tick.delay,
                                tick.priority,
                            )
                        }),
                        ticks.supports_block,
                    )
                    .context("saved block ticks cannot be preserved")?;
                } else if name.matches("fluid_ticks") {
                    fluid_ticks_seen = true;
                    check_tick_list(
                        &value,
                        ticks.position,
                        ticks.fluids.iter().map(|tick| {
                            (
                                tick.fluid.as_str(),
                                tick.position,
                                tick.delay,
                                tick.priority,
                            )
                        }),
                        supports_fluid_tick,
                    )
                    .context("saved fluid ticks cannot be preserved")?;
                } else {
                    bail!("unrecognized preparation field");
                }
            }
        }
    }
    ensure!(
        block_ticks_seen || ticks.blocks.is_empty(),
        "saved block ticks were not captured"
    );
    ensure!(
        fluid_ticks_seen || ticks.fluids.is_empty(),
        "saved fluid ticks were not captured"
    );
    match entities {
        SavedBlockEntities::Empty => {}
        SavedBlockEntities::Unavailable => bail!("saved block entities are unavailable"),
        SavedBlockEntities::Encoded(bytes) => {
            let document = nbt::read(bytes).context("cannot read saved block entities")?;
            let Tag::List {
                element_type: TagType::Compound,
                values,
            } = document.value
            else {
                bail!("saved block entities are not a compound list");
            };
            for entity in values {
                let Tag::Compound(fields) = entity else {
                    bail!("saved block entity is not a compound");
                };
                let mut markers = fields.iter().filter(|(key, _)| key.matches("keepPacked"));
                let marker = markers.next();
                ensure!(markers.next().is_none(), "duplicate keepPacked marker");
                match marker.map(|(_, value)| value) {
                    None | Some(Tag::Byte(0)) => {}
                    Some(Tag::Byte(_)) => bail!("packed block entity needs promotion"),
                    Some(_) => bail!("keepPacked marker has an unsupported type"),
                }
            }
        }
    }
    Ok(marks)
}

fn supports_fluid_tick(id: &str) -> bool {
    matches!(
        id,
        "minecraft:water"
            | "water"
            | "minecraft:flowing_water"
            | "flowing_water"
            | "minecraft:lava"
            | "lava"
            | "minecraft:flowing_lava"
            | "flowing_lava"
    )
}

fn check_tick_list<'a>(
    tag: &Tag,
    chunk: ChunkPos,
    expected: impl ExactSizeIterator<Item = (&'a str, [i32; 3], i32, i32)>,
    supported: fn(&str) -> bool,
) -> Result<()> {
    let Tag::List {
        element_type,
        values,
    } = tag
    else {
        bail!("saved tick data is not a list");
    };
    ensure!(
        *element_type == TagType::Compound || (*element_type == TagType::End && values.is_empty()),
        "saved tick list has an unsupported element type"
    );
    ensure!(
        values.len() == expected.len(),
        "saved tick list differs from the loaded ticks"
    );
    for (entry, (expected_id, expected_position, expected_delay, expected_priority)) in
        values.iter().zip(expected)
    {
        let Tag::Compound(fields) = entry else {
            bail!("saved tick entry is not a compound");
        };
        ensure!(fields.len() == 6, "saved tick entry has unsupported fields");
        let mut names = BTreeSet::new();
        for (name, _) in fields {
            ensure!(names.insert(name.0.clone()), "duplicate saved tick field");
            ensure!(
                ["i", "x", "y", "z", "t", "p"]
                    .iter()
                    .any(|candidate| name.matches(candidate)),
                "unrecognized saved tick field"
            );
        }
        let Some(Tag::String(id)) = entry.get("i") else {
            bail!("saved tick id is missing");
        };
        ensure!(
            id.matches(expected_id),
            "saved tick id differs from loaded tick"
        );
        ensure!(
            supported(expected_id),
            "unsupported saved tick {expected_id}"
        );
        for (name, expected) in [
            ("x", expected_position[0]),
            ("y", expected_position[1]),
            ("z", expected_position[2]),
            ("t", expected_delay),
            ("p", expected_priority),
        ] {
            ensure!(
                entry.get(name).and_then(Tag::as_java_int) == Some(expected),
                "saved tick {name} differs from loaded tick"
            );
        }
        ensure!(
            expected_position[0].div_euclid(16) == chunk.x
                && expected_position[2].div_euclid(16) == chunk.z,
            "saved tick belongs to another chunk"
        );
        ensure!(
            (-3..=3).contains(&expected_priority),
            "saved tick priority is unsupported"
        );
    }
    Ok(())
}

fn check_upgrade_data(tag: &Tag, bounds: BuildBounds) -> Result<()> {
    let Tag::Compound(fields) = tag else {
        bail!("saved upgrade data is not a compound");
    };
    let height = bounds
        .max_y
        .checked_sub(bounds.min_y)
        .context("invalid dimension height")?;
    ensure!(height > 0 && height % 16 == 0, "invalid dimension height");
    let mut seen = BTreeSet::new();
    for (name, value) in fields {
        ensure!(seen.insert(name.0.clone()), "duplicate upgrade data field");
        if name.matches("Sides") {
            ensure!(value.as_java_int() == Some(0), "upgrade sides need updates");
        } else if name.matches("Indices") {
            let Tag::Compound(indices) = value else {
                bail!("upgrade indices are not a compound");
            };
            let mut sections = BTreeSet::new();
            for (name, value) in indices {
                ensure!(sections.insert(name.0.clone()), "duplicate upgrade section");
                let text = String::from_utf16(&name.0).context("invalid upgrade section")?;
                let index: i32 = text.parse().context("invalid upgrade section")?;
                ensure!(
                    index >= 0 && index < height / 16 && index.to_string() == text,
                    "upgrade section is outside the dimension"
                );
                ensure!(
                    matches!(value, Tag::IntArray(indices) if indices.is_empty()),
                    "upgrade indices need updates"
                );
            }
        } else if name.matches("neighbor_block_ticks") || name.matches("neighbor_fluid_ticks") {
            ensure!(
                matches!(value, Tag::List { element_type: TagType::End | TagType::Compound, values } if values.is_empty()),
                "upgrade neighbor ticks need scheduling"
            );
        } else {
            bail!("unrecognized upgrade data field");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::NamedTag;

    fn encode(tag: Tag) -> std::sync::Arc<[u8]> {
        nbt::write(&NamedTag {
            name: "".into(),
            value: tag,
        })
        .unwrap()
        .into()
    }

    fn no_ticks() -> SavedTickInputs<'static> {
        SavedTickInputs {
            position: ChunkPos { x: 0, z: 0 },
            blocks: &[],
            fluids: &[],
            supports_block: |_| false,
        }
    }

    #[test]
    fn unbrushed_reset_requires_resolved_matching_entity_data() {
        let position = [1, 2, 3];
        let base = vec![
            ("id".into(), Tag::String("minecraft:brushable_block".into())),
            ("x".into(), Tag::Int(1)),
            ("y".into(), Tag::Int(2)),
            ("z".into(), Tag::Int(3)),
            (
                "LootTable".into(),
                Tag::String("minecraft:archaeology/ocean_ruin_cold".into()),
            ),
        ];
        let payload = |values| {
            SavedBlockEntities::Encoded(encode(Tag::List {
                element_type: TagType::Compound,
                values,
            }))
        };
        assert!(brushable_reset_is_inert(
            &SavedBlockEntities::Empty,
            position
        ));
        assert!(!brushable_reset_is_inert(
            &SavedBlockEntities::Unavailable,
            position
        ));
        assert!(brushable_reset_is_inert(
            &payload(vec![Tag::Compound(base.clone())]),
            position
        ));
        let mut hit = base.clone();
        hit.push(("hit_direction".into(), Tag::Int(2)));
        assert!(!brushable_reset_is_inert(
            &payload(vec![Tag::Compound(hit)]),
            position
        ));
        let mut packed = base.clone();
        packed.push(("keepPacked".into(), Tag::Byte(1)));
        assert!(!brushable_reset_is_inert(
            &payload(vec![Tag::Compound(packed)]),
            position
        ));
        let mut duplicate_field = base.clone();
        duplicate_field.push(("x".into(), Tag::Int(1)));
        assert!(!brushable_reset_is_inert(
            &payload(vec![Tag::Compound(duplicate_field)]),
            position
        ));
        assert!(!brushable_reset_is_inert(
            &payload(vec![Tag::Compound(base.clone()), Tag::Compound(base)]),
            position,
        ));
    }

    #[test]
    fn detects_saved_preparation_work_without_claiming_chunk_readiness() {
        let bounds = BuildBounds {
            min_y: -16,
            max_y: 16,
        };
        let empty_sections = Tag::List {
            element_type: TagType::List,
            values: vec![
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ],
        };
        let check = |post: Tag, upgrade: Tag| {
            check_saved_work(
                &SavedChunkActivation::Encoded(encode(Tag::Compound(vec![
                    ("PostProcessing".into(), post),
                    ("UpgradeData".into(), upgrade),
                ]))),
                &SavedBlockEntities::Empty,
                bounds,
                no_ticks(),
            )
        };
        let upgrade = Tag::Compound(vec![
            ("Sides".into(), Tag::Byte(0)),
            (
                "Indices".into(),
                Tag::Compound(vec![("0".into(), Tag::IntArray(vec![]))]),
            ),
            (
                "neighbor_block_ticks".into(),
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ),
        ]);
        assert!(check(empty_sections.clone(), upgrade.clone()).is_ok());
        let mut pending = empty_sections.clone();
        if let Tag::List { values, .. } = &mut pending {
            values[0] = Tag::List {
                element_type: TagType::Short,
                values: vec![Tag::Short(0)],
            };
        }
        assert!(
            check(pending, upgrade.clone())
                .unwrap_err()
                .to_string()
                .contains("postprocessing")
        );
        assert!(
            check(
                empty_sections.clone(),
                Tag::Compound(vec![("Sides".into(), Tag::Byte(1))])
            )
            .is_err()
        );
        assert!(
            check(
                empty_sections.clone(),
                Tag::Compound(vec![(
                    "Indices".into(),
                    Tag::Compound(vec![("0".into(), Tag::IntArray(vec![1]))])
                )])
            )
            .is_err()
        );
        assert!(
            check(
                empty_sections.clone(),
                Tag::Compound(vec![(
                    "neighbor_fluid_ticks".into(),
                    Tag::List {
                        element_type: TagType::Compound,
                        values: vec![Tag::Compound(vec![])]
                    }
                )])
            )
            .is_err()
        );
        assert!(
            check(
                empty_sections.clone(),
                Tag::Compound(vec![("unknown".into(), Tag::Byte(0))])
            )
            .is_err()
        );
        assert!(check(Tag::Int(0), upgrade).is_err());
        assert!(
            check_saved_work(
                &SavedChunkActivation::Unavailable,
                &SavedBlockEntities::Empty,
                bounds,
                no_ticks(),
            )
            .is_err()
        );
    }

    #[test]
    fn detects_packed_or_ambiguous_block_entities() {
        let bounds = BuildBounds {
            min_y: 0,
            max_y: 16,
        };
        let check = |fields| {
            check_saved_work(
                &SavedChunkActivation::Empty,
                &SavedBlockEntities::Encoded(encode(Tag::List {
                    element_type: TagType::Compound,
                    values: vec![Tag::Compound(fields)],
                })),
                bounds,
                no_ticks(),
            )
        };
        assert!(check(vec![]).is_ok());
        assert!(check(vec![("keepPacked".into(), Tag::Byte(0))]).is_ok());
        assert!(check(vec![("keepPacked".into(), Tag::Byte(1))]).is_err());
        assert!(check(vec![("keepPacked".into(), Tag::Int(0))]).is_err());
        assert!(
            check(vec![
                ("keepPacked".into(), Tag::Byte(0)),
                ("keepPacked".into(), Tag::Byte(1))
            ])
            .is_err()
        );
    }

    #[test]
    fn validates_the_complete_original_tick_lists() {
        let position = ChunkPos { x: -1, z: 1 };
        let blocks = vec![SavedBlockTick {
            block: "minecraft:sand".into(),
            position: [-1, 4, 16],
            delay: -3,
            priority: 2,
        }];
        let fluids = vec![SavedFluidTick {
            fluid: "minecraft:water".into(),
            position: [-2, 6, 17],
            delay: 12,
            priority: -1,
        }];
        let tick = |id: &str, [x, y, z]: [i32; 3], delay, priority| {
            Tag::Compound(vec![
                ("i".into(), Tag::String(id.into())),
                ("x".into(), Tag::Int(x)),
                ("y".into(), Tag::Int(y)),
                ("z".into(), Tag::Int(z)),
                ("t".into(), Tag::Int(delay)),
                ("p".into(), Tag::Int(priority)),
            ])
        };
        let list = |values| Tag::List {
            element_type: TagType::Compound,
            values,
        };
        let block = tick("minecraft:sand", blocks[0].position, -3, 2);
        let fluid = tick("minecraft:water", fluids[0].position, 12, -1);
        let check = |block_data: Option<Tag>, fluid_data: Option<Tag>| {
            let mut fields = Vec::new();
            if let Some(data) = block_data {
                fields.push(("block_ticks".into(), data));
            }
            if let Some(data) = fluid_data {
                fields.push(("fluid_ticks".into(), data));
            }
            check_saved_work(
                &SavedChunkActivation::Encoded(encode(Tag::Compound(fields))),
                &SavedBlockEntities::Empty,
                BuildBounds {
                    min_y: 0,
                    max_y: 16,
                },
                SavedTickInputs {
                    position,
                    blocks: &blocks,
                    fluids: &fluids,
                    supports_block: |id| id == "minecraft:sand",
                },
            )
        };
        assert!(
            check(
                Some(list(vec![block.clone()])),
                Some(list(vec![fluid.clone()]))
            )
            .is_ok()
        );
        assert!(check(None, Some(list(vec![fluid.clone()]))).is_err());
        assert!(check(Some(list(vec![])), Some(list(vec![fluid.clone()]))).is_err());

        let mut extra = block.clone();
        if let Tag::Compound(fields) = &mut extra {
            fields.push(("extra".into(), Tag::Int(1)));
        }
        assert!(check(Some(list(vec![extra])), Some(list(vec![fluid.clone()]))).is_err());
        let out_of_range_priority = tick("minecraft:water", fluids[0].position, 12, 4);
        assert!(
            check(
                Some(list(vec![block.clone()])),
                Some(list(vec![out_of_range_priority]))
            )
            .is_err()
        );
        let wrong_chunk = tick("minecraft:sand", [0, 4, 16], -3, 2);
        assert!(check(Some(list(vec![wrong_chunk])), Some(list(vec![fluid]))).is_err());
    }
}
