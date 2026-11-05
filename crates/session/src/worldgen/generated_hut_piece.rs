//! Strict NBT representation of a fresh swamp-hut structure piece.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::{
    structure_piece::{ScatteredGroundHeight, swamp_hut::SwampHutState},
    structure_start::{HorizontalDirection, SimpleStructureKind, SimpleStructureStart},
};

/// State needed to resume the same hut across decorated chunks. This does not
/// authorize reading or writing an existing world.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SavedSwampHutPiece {
    pub bounds: BoundingBox,
    pub orientation: HorizontalDirection,
    pub state: SwampHutState,
}

impl SavedSwampHutPiece {
    pub fn capture(piece: &SimpleStructureStart, state: SwampHutState) -> Result<Self> {
        ensure!(
            piece.kind == SimpleStructureKind::SwampHut,
            "piece is not a swamp hut"
        );
        let saved = Self {
            bounds: piece.bounds,
            orientation: piece.orientation.context("hut orientation is missing")?,
            state,
        };
        saved.validate()?;
        Ok(saved)
    }

    fn validate(&self) -> Result<()> {
        let size =
            |axis: usize| i64::from(self.bounds.max[axis]) - i64::from(self.bounds.min[axis]) + 1;
        let (width, depth) = match self.orientation {
            HorizontalDirection::North | HorizontalDirection::South => (7, 9),
            HorizontalDirection::East | HorizontalDirection::West => (9, 7),
        };
        ensure!(
            size(0) == width && size(1) == 7 && size(2) == depth,
            "invalid swamp hut bounds"
        );
        Ok(())
    }

    pub fn to_tag(&self) -> Result<Tag> {
        self.validate()?;
        let orientation = match self.orientation {
            HorizontalDirection::South => 0,
            HorizontalDirection::West => 1,
            HorizontalDirection::North => 2,
            HorizontalDirection::East => 3,
        };
        Ok(Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:tesh".into())),
            (
                "BB".into(),
                Tag::IntArray(self.bounds.min.into_iter().chain(self.bounds.max).collect()),
            ),
            ("O".into(), Tag::Int(orientation)),
            ("GD".into(), Tag::Int(0)),
            ("Width".into(), Tag::Int(7)),
            ("Height".into(), Tag::Int(7)),
            ("Depth".into(), Tag::Int(9)),
            ("HPos".into(), Tag::Int(self.state.ground.height_position())),
            (
                "Witch".into(),
                Tag::Byte(i8::from(self.state.spawned_witch)),
            ),
            ("Cat".into(), Tag::Byte(i8::from(self.state.spawned_cat))),
        ]))
    }

    /// Decode only the exact generated representation; unknown or duplicate
    /// fields cannot be safely preserved by this draft and are rejected.
    pub fn from_tag(tag: &Tag) -> Result<Self> {
        let Tag::Compound(fields) = tag else {
            anyhow::bail!("hut piece must be a compound")
        };
        const NAMES: [&str; 10] = [
            "id", "BB", "O", "GD", "Width", "Height", "Depth", "HPos", "Witch", "Cat",
        ];
        ensure!(
            fields.len() == NAMES.len()
                && NAMES.iter().all(|name| fields
                    .iter()
                    .filter(|(key, _)| key.matches(name))
                    .count()
                    == 1),
            "hut piece has unknown, missing, or duplicate fields"
        );
        ensure!(
            matches!(tag.get("id"), Some(Tag::String(id)) if id.matches("minecraft:tesh")),
            "unexpected hut piece id"
        );
        let integer = |name: &str| -> Result<i32> {
            match tag.get(name) {
                Some(Tag::Int(value)) => Ok(*value),
                _ => anyhow::bail!("invalid hut field {name}"),
            }
        };
        let flag = |name: &str| -> Result<bool> {
            match tag.get(name) {
                Some(Tag::Byte(0)) => Ok(false),
                Some(Tag::Byte(1)) => Ok(true),
                _ => anyhow::bail!("invalid hut flag {name}"),
            }
        };
        ensure!(
            integer("GD")? == 0
                && integer("Width")? == 7
                && integer("Height")? == 7
                && integer("Depth")? == 9,
            "unsupported hut dimensions or depth"
        );
        let orientation = match integer("O")? {
            0 => HorizontalDirection::South,
            1 => HorizontalDirection::West,
            2 => HorizontalDirection::North,
            3 => HorizontalDirection::East,
            _ => anyhow::bail!("invalid hut orientation"),
        };
        let Some(Tag::IntArray(b)) = tag.get("BB") else {
            anyhow::bail!("invalid hut bounding box")
        };
        ensure!(b.len() == 6, "invalid hut bounding box size");
        let saved = Self {
            bounds: BoundingBox::new([b[0], b[1], b[2]], [b[3], b[4], b[5]])
                .map_err(anyhow::Error::msg)?,
            orientation,
            state: SwampHutState {
                ground: ScatteredGroundHeight::from_saved(integer("HPos")?),
                spawned_witch: flag("Witch")?,
                spawned_cat: flag("Cat")?,
            },
        };
        saved.validate()?;
        Ok(saved)
    }

    /// Restore only onto the same hut, preserving its start metadata.
    pub fn restore(&self, piece: &mut SimpleStructureStart) -> Result<SwampHutState> {
        self.validate()?;
        ensure!(
            piece.kind == SimpleStructureKind::SwampHut
                && piece.orientation == Some(self.orientation),
            "different swamp hut"
        );
        ensure!(
            [0, 2]
                .into_iter()
                .all(|axis| piece.bounds.min[axis] == self.bounds.min[axis]
                    && piece.bounds.max[axis] == self.bounds.max[axis]),
            "swamp hut horizontal bounds differ"
        );
        piece.bounds = self.bounds;
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::beardifier::TerrainAdjustment;

    #[test]
    fn rejects_unknown_fields_and_restores_only_matching_hut() {
        let mut piece = SimpleStructureStart {
            kind: SimpleStructureKind::SwampHut,
            chunk: [0, 0],
            generation_point: [0, 0, 0],
            orientation: Some(HorizontalDirection::South),
            bounds: BoundingBox::new([3, 64, 3], [9, 70, 11]).unwrap(),
            terrain_adjustment: TerrainAdjustment::None,
        };
        let state = SwampHutState {
            ground: ScatteredGroundHeight::from_saved(70),
            spawned_witch: true,
            spawned_cat: false,
        };
        let mut aligned = piece.clone();
        aligned.bounds = BoundingBox::new([3, 70, 3], [9, 76, 11]).unwrap();
        let saved = SavedSwampHutPiece::capture(&aligned, state).unwrap();
        let tag = saved.to_tag().unwrap();
        assert_eq!(SavedSwampHutPiece::from_tag(&tag).unwrap(), saved);
        assert_eq!(saved.restore(&mut piece).unwrap(), state);
        assert_eq!(piece.bounds, aligned.bounds);
        piece.bounds.min[0] -= 1;
        assert!(saved.restore(&mut piece).is_err());
        let Tag::Compound(mut fields) = tag else {
            unreachable!()
        };
        fields.push(("future".into(), Tag::Int(1)));
        assert!(SavedSwampHutPiece::from_tag(&Tag::Compound(fields)).is_err());
    }
}
