//! Checks for saved preparation work that the live session cannot yet run.
//! Passing this check does not establish Java chunk-holder or ticket readiness.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use native_persistence::nbt::{self, Tag, TagType};
use native_world::loaded::{SavedBlockEntities, SavedChunkActivation};

use crate::loaded::BuildBounds;

pub(crate) fn check_saved_work(
    saved: &SavedChunkActivation,
    entities: &SavedBlockEntities,
    bounds: BuildBounds,
) -> Result<()> {
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
                    check_postprocessing(&value, bounds)?;
                } else if name.matches("UpgradeData") {
                    check_upgrade_data(&value, bounds)?;
                } else {
                    bail!("unrecognized preparation field");
                }
            }
        }
    }
    match entities {
        SavedBlockEntities::Empty => Ok(()),
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
            Ok(())
        }
    }
}

fn check_postprocessing(tag: &Tag, bounds: BuildBounds) -> Result<()> {
    if matches!(tag, Tag::List { element_type: TagType::End | TagType::List, values } if values.is_empty())
    {
        return Ok(());
    }
    let height = bounds
        .max_y
        .checked_sub(bounds.min_y)
        .context("invalid dimension height")?;
    let marks = native_persistence::postprocessing::decode(tag, bounds.min_y, height)?;
    ensure!(
        marks.sections().iter().all(Vec::is_empty),
        "saved postprocessing needs block and fluid updates"
    );
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
                bounds
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
}
