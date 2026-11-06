//! Strict fresh-mineshaft piece NBT. This does not authorize a world save.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::{Tag, TagType};
use native_resources::{
    mineshaft::{MineshaftGraphPiece, MineshaftGraphPieceKind, MineshaftVariant},
    structure_start::HorizontalDirection,
};

/// Includes mutable decoration state needed when a corridor spans chunks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedMineshaftPiece {
    pub piece: MineshaftGraphPiece,
    pub variant: MineshaftVariant,
    pub placed_spider: bool,
}

fn direction_id(direction: HorizontalDirection) -> i32 {
    match direction {
        HorizontalDirection::South => 0,
        HorizontalDirection::West => 1,
        HorizontalDirection::North => 2,
        HorizontalDirection::East => 3,
    }
}

fn direction(value: i32) -> Result<HorizontalDirection> {
    Ok(match value {
        0 => HorizontalDirection::South,
        1 => HorizontalDirection::West,
        2 => HorizontalDirection::North,
        3 => HorizontalDirection::East,
        _ => anyhow::bail!("invalid mineshaft orientation"),
    })
}

fn box_tag(bounds: BoundingBox) -> Tag {
    Tag::IntArray(bounds.min.into_iter().chain(bounds.max).collect())
}

fn read_box(tag: &Tag) -> Result<BoundingBox> {
    let Tag::IntArray(values) = tag else {
        anyhow::bail!("invalid mineshaft bounding box")
    };
    ensure!(values.len() == 6, "invalid mineshaft bounding box length");
    BoundingBox::new(
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
    )
    .map_err(anyhow::Error::msg)
}

impl SavedMineshaftPiece {
    pub fn capture(
        piece: &MineshaftGraphPiece,
        variant: MineshaftVariant,
        placed_spider: bool,
    ) -> Result<Self> {
        let saved = Self {
            piece: piece.clone(),
            variant,
            placed_spider,
        };
        saved.validate()?;
        Ok(saved)
    }

    fn validate(&self) -> Result<()> {
        let p = &self.piece;
        let size = |axis: usize| i64::from(p.bounds.max[axis]) - i64::from(p.bounds.min[axis]) + 1;
        ensure!(
            (0..3).all(|axis| size(axis) > 0),
            "invalid mineshaft bounds"
        );
        match &p.kind {
            MineshaftGraphPieceKind::Room { entrances } => {
                ensure!(
                    p.direction.is_none() && p.depth == 0 && !self.placed_spider,
                    "invalid mineshaft room state"
                );
                ensure!(
                    (8..=13).contains(&size(0))
                        && (5..=10).contains(&size(1))
                        && (8..=13).contains(&size(2)),
                    "invalid mineshaft room bounds"
                );
                for entrance in entrances {
                    ensure!(
                        (0..3).all(|axis| entrance.min[axis] <= entrance.max[axis]),
                        "invalid mineshaft entrance"
                    );
                }
            }
            MineshaftGraphPieceKind::Corridor {
                rails,
                spiders,
                sections,
            } => {
                let d = p.direction.context("corridor orientation missing")?;
                ensure!(
                    (1..=9).contains(&p.depth)
                        && (!self.placed_spider || *spiders)
                        && !(*rails && *spiders),
                    "invalid corridor state"
                );
                let axis = if matches!(d, HorizontalDirection::North | HorizontalDirection::South) {
                    2
                } else {
                    0
                };
                ensure!(
                    (1..=4).contains(sections)
                        && size(axis) == i64::from(*sections) * 5
                        && size(2 - axis) == 3
                        && size(1) == 3,
                    "invalid corridor bounds"
                );
            }
            MineshaftGraphPieceKind::Crossing { two_floors } => {
                ensure!(
                    p.direction.is_some() && (1..=9).contains(&p.depth) && !self.placed_spider,
                    "invalid crossing state"
                );
                ensure!(
                    size(0) == 5 && size(2) == 5 && size(1) == if *two_floors { 7 } else { 3 },
                    "invalid crossing bounds"
                );
            }
            MineshaftGraphPieceKind::Stairs => {
                let d = p.direction.context("stairs orientation missing")?;
                let axis = if matches!(d, HorizontalDirection::North | HorizontalDirection::South) {
                    2
                } else {
                    0
                };
                ensure!(
                    (1..=9).contains(&p.depth)
                        && !self.placed_spider
                        && size(axis) == 9
                        && size(2 - axis) == 3
                        && size(1) == 8,
                    "invalid stairs state"
                );
            }
        }
        Ok(())
    }

    pub fn to_tag(&self) -> Result<Tag> {
        self.validate()?;
        let p = &self.piece;
        let (id, orientation) = match &p.kind {
            MineshaftGraphPieceKind::Room { .. } => ("minecraft:msroom", -1),
            MineshaftGraphPieceKind::Corridor { .. } => (
                "minecraft:mscorridor",
                direction_id(p.direction.context("missing direction")?),
            ),
            MineshaftGraphPieceKind::Crossing { .. } => ("minecraft:mscrossing", -1),
            MineshaftGraphPieceKind::Stairs => (
                "minecraft:msstairs",
                direction_id(p.direction.context("missing direction")?),
            ),
        };
        let mut fields = vec![
            ("id".into(), Tag::String(id.into())),
            ("BB".into(), box_tag(p.bounds)),
            ("O".into(), Tag::Int(orientation)),
            ("GD".into(), Tag::Int(p.depth)),
            (
                "MST".into(),
                Tag::Int(if self.variant == MineshaftVariant::Normal {
                    0
                } else {
                    1
                }),
            ),
        ];
        match &p.kind {
            MineshaftGraphPieceKind::Room { entrances } => fields.push((
                "Entrances".into(),
                Tag::List {
                    element_type: if entrances.is_empty() {
                        TagType::End
                    } else {
                        TagType::IntArray
                    },
                    values: entrances.iter().copied().map(box_tag).collect(),
                },
            )),
            MineshaftGraphPieceKind::Corridor {
                rails,
                spiders,
                sections,
            } => fields.extend([
                ("hr".into(), Tag::Byte(i8::from(*rails))),
                ("sc".into(), Tag::Byte(i8::from(*spiders))),
                ("hps".into(), Tag::Byte(i8::from(self.placed_spider))),
                ("Num".into(), Tag::Int(*sections)),
            ]),
            MineshaftGraphPieceKind::Crossing { two_floors } => fields.extend([
                ("tf".into(), Tag::Byte(i8::from(*two_floors))),
                (
                    "D".into(),
                    Tag::Byte(direction_id(p.direction.context("missing direction")?) as i8),
                ),
            ]),
            MineshaftGraphPieceKind::Stairs => {}
        }
        Ok(Tag::Compound(fields))
    }

    /// Decode only the represented fields; unknown data must not be discarded.
    pub fn from_tag(tag: &Tag) -> Result<Self> {
        let Tag::Compound(fields) = tag else {
            anyhow::bail!("mineshaft piece must be a compound")
        };
        let Some(Tag::String(id)) = tag.get("id") else {
            anyhow::bail!("missing mineshaft piece id")
        };
        let names: &[&str] = if id.matches("minecraft:msroom") {
            &["id", "BB", "O", "GD", "MST", "Entrances"]
        } else if id.matches("minecraft:mscorridor") {
            &["id", "BB", "O", "GD", "MST", "hr", "sc", "hps", "Num"]
        } else if id.matches("minecraft:mscrossing") {
            &["id", "BB", "O", "GD", "MST", "tf", "D"]
        } else if id.matches("minecraft:msstairs") {
            &["id", "BB", "O", "GD", "MST"]
        } else {
            anyhow::bail!("unsupported mineshaft piece id")
        };
        ensure!(
            fields.len() == names.len()
                && names.iter().all(|name| fields
                    .iter()
                    .filter(|(key, _)| key.matches(name))
                    .count()
                    == 1),
            "unknown, missing, or duplicate mineshaft fields"
        );
        let integer = |name: &str| -> Result<i32> {
            match tag.get(name) {
                Some(Tag::Int(v)) => Ok(*v),
                _ => anyhow::bail!("invalid mineshaft field {name}"),
            }
        };
        let flag = |name: &str| -> Result<bool> {
            match tag.get(name) {
                Some(Tag::Byte(0)) => Ok(false),
                Some(Tag::Byte(1)) => Ok(true),
                _ => anyhow::bail!("invalid mineshaft flag {name}"),
            }
        };
        let variant = match integer("MST")? {
            0 => MineshaftVariant::Normal,
            1 => MineshaftVariant::Mesa,
            _ => anyhow::bail!("invalid mineshaft variant"),
        };
        let bounds = read_box(tag.get("BB").context("missing bounds")?)?;
        let depth = integer("GD")?;
        let orientation = integer("O")?;
        let mut placed_spider = false;
        let (piece_direction, kind) = if id.matches("minecraft:msroom") {
            ensure!(orientation == -1, "room must not have orientation");
            let Some(Tag::List {
                element_type,
                values,
            }) = tag.get("Entrances")
            else {
                anyhow::bail!("invalid mineshaft entrances")
            };
            ensure!(
                *element_type == TagType::IntArray
                    || (values.is_empty() && *element_type == TagType::End),
                "invalid mineshaft entrance list"
            );
            (
                None,
                MineshaftGraphPieceKind::Room {
                    entrances: values.iter().map(read_box).collect::<Result<Vec<_>>>()?,
                },
            )
        } else if id.matches("minecraft:mscorridor") {
            placed_spider = flag("hps")?;
            (
                Some(direction(orientation)?),
                MineshaftGraphPieceKind::Corridor {
                    rails: flag("hr")?,
                    spiders: flag("sc")?,
                    sections: integer("Num")?,
                },
            )
        } else if id.matches("minecraft:mscrossing") {
            ensure!(orientation == -1, "crossing must not have base orientation");
            let Some(Tag::Byte(value)) = tag.get("D") else {
                anyhow::bail!("invalid crossing direction")
            };
            (
                Some(direction(i32::from(*value))?),
                MineshaftGraphPieceKind::Crossing {
                    two_floors: flag("tf")?,
                },
            )
        } else {
            (
                Some(direction(orientation)?),
                MineshaftGraphPieceKind::Stairs,
            )
        };
        let saved = Self {
            piece: MineshaftGraphPiece {
                bounds,
                direction: piece_direction,
                depth,
                kind,
            },
            variant,
            placed_spider,
        };
        saved.validate()?;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spider_corridor() -> SavedMineshaftPiece {
        SavedMineshaftPiece {
            variant: MineshaftVariant::Mesa,
            placed_spider: true,
            piece: MineshaftGraphPiece {
                bounds: BoundingBox::new([10, 20, 30], [12, 22, 39]).unwrap(),
                direction: Some(HorizontalDirection::North),
                depth: 3,
                kind: MineshaftGraphPieceKind::Corridor {
                    rails: false,
                    spiders: true,
                    sections: 2,
                },
            },
        }
    }

    #[test]
    fn preserves_spawner_state_and_rejects_unknown_data() {
        let saved = spider_corridor();
        let tag = saved.to_tag().unwrap();
        assert_eq!(SavedMineshaftPiece::from_tag(&tag).unwrap(), saved);
        let Tag::Compound(mut fields) = tag else {
            unreachable!()
        };
        fields.push(("future_data".into(), Tag::Int(1)));
        assert!(SavedMineshaftPiece::from_tag(&Tag::Compound(fields)).is_err());
    }

    #[test]
    fn rejects_inconsistent_corridor_state_and_geometry() {
        let mut saved = spider_corridor();
        saved.piece.bounds.max[2] += 1;
        assert!(saved.to_tag().is_err());
        let mut saved = spider_corridor();
        if let MineshaftGraphPieceKind::Corridor { spiders, .. } = &mut saved.piece.kind {
            *spiders = false;
        }
        assert!(saved.to_tag().is_err());
    }
}
