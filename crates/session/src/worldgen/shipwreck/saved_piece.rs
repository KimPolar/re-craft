//! Strict representation of a generated Java 26.3 shipwreck piece.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::{packs::ResourceId, shipwreck::Piece, structure_transform::Rotation};

use super::PreparedStart;

#[derive(Clone, Debug, PartialEq)]
pub struct SavedShipwreckPiece {
    pub piece: Piece,
    pub bounds: BoundingBox,
    pub height_adjusted: bool,
    template_size: [i32; 3],
}

impl SavedShipwreckPiece {
    pub fn capture(start: &PreparedStart) -> Result<Self> {
        let saved = Self {
            piece: start.piece.clone(),
            bounds: start.bounds,
            height_adjusted: start.height_adjusted,
            template_size: start.template.size,
        };
        saved.validate()?;
        Ok(saved)
    }

    fn validate(&self) -> Result<()> {
        let id = ResourceId::parse(&self.piece.template)?;
        ensure!(
            format!("{}:{}", id.namespace, id.path) == self.piece.template,
            "noncanonical shipwreck template identifier"
        );
        ensure!(
            self.template_size.iter().all(|v| *v > 0),
            "invalid shipwreck template size"
        );
        let current = self.piece.bounds(self.template_size)?;
        let mut original = self.piece.clone();
        original.position[1] = 90;
        let before_placement = original.bounds(self.template_size)?;
        // Oversized pieces can be adjusted before TemplateStructurePiece
        // refreshes its cached bounding box in postProcess.
        ensure!(
            self.bounds == current
                || (self.height_adjusted
                    && native_resources::shipwreck::too_big_for_region(self.template_size)
                    && self.bounds == before_placement),
            "unsupported shipwreck bounds"
        );
        Ok(())
    }

    pub fn to_tag(&self) -> Result<Tag> {
        self.validate()?;
        let rotation = match self.piece.rotation {
            Rotation::None => "NONE",
            Rotation::Clockwise90 => "CLOCKWISE_90",
            Rotation::Clockwise180 => "CLOCKWISE_180",
            Rotation::Counterclockwise90 => "COUNTERCLOCKWISE_90",
        };
        let [x, y, z] = self.piece.position;
        Ok(Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:shipwreck".into())),
            (
                "BB".into(),
                Tag::IntArray(self.bounds.min.into_iter().chain(self.bounds.max).collect()),
            ),
            ("O".into(), Tag::Int(2)),
            ("GD".into(), Tag::Int(0)),
            ("TPX".into(), Tag::Int(x)),
            ("TPY".into(), Tag::Int(y)),
            ("TPZ".into(), Tag::Int(z)),
            (
                "Template".into(),
                Tag::String(self.piece.template.as_str().into()),
            ),
            ("Rot".into(), Tag::String(rotation.into())),
            ("isBeached".into(), Tag::Byte(i8::from(self.piece.beached))),
            (
                "height_adjusted".into(),
                Tag::Byte(i8::from(self.height_adjusted)),
            ),
        ]))
    }

    /// Refuse unknown or lossy data; callers supply the active template size.
    pub fn from_tag(tag: &Tag, template_size: [i32; 3]) -> Result<Self> {
        let Tag::Compound(fields) = tag else {
            anyhow::bail!("shipwreck piece must be a compound")
        };
        const NAMES: [&str; 11] = [
            "id",
            "BB",
            "O",
            "GD",
            "TPX",
            "TPY",
            "TPZ",
            "Template",
            "Rot",
            "isBeached",
            "height_adjusted",
        ];
        ensure!(
            fields.len() == NAMES.len()
                && NAMES.iter().all(|name| fields
                    .iter()
                    .filter(|(key, _)| key.matches(name))
                    .count()
                    == 1),
            "shipwreck piece has unknown, missing or duplicate fields"
        );
        let text = |name: &str| -> Result<String> {
            match tag.get(name) {
                Some(Tag::String(value)) => String::from_utf16(&value.0)
                    .with_context(|| format!("invalid shipwreck field {name}")),
                _ => anyhow::bail!("invalid shipwreck field {name}"),
            }
        };
        let integer = |name: &str| -> Result<i32> {
            match tag.get(name) {
                Some(Tag::Int(value)) => Ok(*value),
                _ => anyhow::bail!("invalid shipwreck field {name}"),
            }
        };
        let boolean = |name: &str| -> Result<bool> {
            match tag.get(name) {
                Some(Tag::Byte(0)) => Ok(false),
                Some(Tag::Byte(1)) => Ok(true),
                _ => anyhow::bail!("invalid shipwreck field {name}"),
            }
        };
        ensure!(
            text("id")? == "minecraft:shipwreck" && integer("O")? == 2 && integer("GD")? == 0,
            "unsupported shipwreck identity or orientation"
        );
        let rotation = match text("Rot")?.as_str() {
            "NONE" => Rotation::None,
            "CLOCKWISE_90" => Rotation::Clockwise90,
            "CLOCKWISE_180" => Rotation::Clockwise180,
            "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
            _ => anyhow::bail!("invalid shipwreck rotation"),
        };
        let Some(Tag::IntArray(bb)) = tag.get("BB") else {
            anyhow::bail!("invalid shipwreck bounds")
        };
        ensure!(bb.len() == 6, "invalid shipwreck bounds length");
        let saved = Self {
            piece: Piece {
                template: text("Template")?,
                position: [integer("TPX")?, integer("TPY")?, integer("TPZ")?],
                rotation,
                beached: boolean("isBeached")?,
            },
            bounds: BoundingBox::new([bb[0], bb[1], bb[2]], [bb[3], bb[4], bb[5]])
                .map_err(anyhow::Error::msg)?,
            height_adjusted: boolean("height_adjusted")?,
            template_size,
        };
        saved.validate()?;
        Ok(saved)
    }

    /// Java reload reconstructs the box from the template origin, even when
    /// the stored box was stale before the first postProcess call.
    pub fn after_java_reload(&self) -> Result<Self> {
        self.validate()?;
        let mut loaded = self.clone();
        loaded.bounds = loaded.piece.bounds(loaded.template_size)?;
        Ok(loaded)
    }
}
