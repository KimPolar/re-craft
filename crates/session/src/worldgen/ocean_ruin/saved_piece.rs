//! Strict 26.3 NBT for a generated ocean-ruin template piece.
use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;
use native_persistence::nbt::Tag;
use native_resources::{
    ocean_ruin::{Piece, Rotation, Temperature},
    packs::ResourceId,
    structure_transform::{Mirror, bounding_box},
};

use super::PreparedPiece;

/// A generated piece and its active template dimensions. This draft is not
/// authorization to write or transform an existing world.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedOceanRuinPiece {
    pub selection: Piece,
    pub temperature: Temperature,
    pub bounds: BoundingBox,
    template_size: [i32; 3],
}

impl SavedOceanRuinPiece {
    pub fn capture(piece: &PreparedPiece, temperature: Temperature) -> Result<Self> {
        let saved = Self {
            selection: piece.selection.clone(),
            temperature,
            bounds: piece.bounds,
            template_size: piece.template.size,
        };
        saved.validate()?;
        Ok(saved)
    }

    fn validate(&self) -> Result<()> {
        let id = ResourceId::parse(&self.selection.template)?;
        ensure!(
            format!("{}:{}", id.namespace, id.path) == self.selection.template,
            "ocean ruin template identifier is not canonical"
        );
        ensure!(
            self.selection.integrity.is_finite() && (0.0..=1.0).contains(&self.selection.integrity),
            "invalid ocean ruin integrity"
        );
        ensure!(
            self.template_size.iter().all(|&v| v > 0),
            "invalid ocean ruin template size"
        );
        let expected = bounding_box(
            self.template_size,
            self.selection.position,
            Mirror::None,
            self.selection.rotation,
            [0; 3],
        )?;
        ensure!(
            self.bounds == expected,
            "ocean ruin bounds differ from template position"
        );
        Ok(())
    }

    pub fn to_tag(&self) -> Result<Tag> {
        self.validate()?;
        let rotation = match self.selection.rotation {
            Rotation::None => "NONE",
            Rotation::Clockwise90 => "CLOCKWISE_90",
            Rotation::Clockwise180 => "CLOCKWISE_180",
            Rotation::Counterclockwise90 => "COUNTERCLOCKWISE_90",
        };
        let temperature = match self.temperature {
            Temperature::Warm => "WARM",
            Temperature::Cold => "COLD",
        };
        let [x, y, z] = self.selection.position;
        Ok(Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:orp".into())),
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
                Tag::String(self.selection.template.as_str().into()),
            ),
            ("Rot".into(), Tag::String(rotation.into())),
            ("Integrity".into(), Tag::Float(self.selection.integrity)),
            ("BiomeType".into(), Tag::String(temperature.into())),
            ("IsLarge".into(), Tag::Byte(i8::from(self.selection.large))),
        ]))
    }

    /// Decode only the exact format this implementation can preserve. The
    /// supplied dimensions must come from the same active template resource.
    pub fn from_tag(tag: &Tag, template_size: [i32; 3]) -> Result<Self> {
        let Tag::Compound(fields) = tag else {
            anyhow::bail!("ocean ruin piece must be a compound")
        };
        const NAMES: [&str; 12] = [
            "id",
            "BB",
            "O",
            "GD",
            "TPX",
            "TPY",
            "TPZ",
            "Template",
            "Rot",
            "Integrity",
            "BiomeType",
            "IsLarge",
        ];
        ensure!(
            fields.len() == NAMES.len()
                && NAMES.iter().all(|name| fields
                    .iter()
                    .filter(|(key, _)| key.matches(name))
                    .count()
                    == 1),
            "ocean ruin piece has unknown, missing, or duplicate fields"
        );
        let text = |name: &str| -> Result<String> {
            match tag.get(name) {
                Some(Tag::String(value)) => String::from_utf16(&value.0)
                    .with_context(|| format!("invalid ocean ruin field {name}")),
                _ => anyhow::bail!("invalid ocean ruin field {name}"),
            }
        };
        let integer = |name: &str| -> Result<i32> {
            match tag.get(name) {
                Some(Tag::Int(value)) => Ok(*value),
                _ => anyhow::bail!("invalid ocean ruin field {name}"),
            }
        };
        ensure!(
            text("id")? == "minecraft:orp" && integer("O")? == 2 && integer("GD")? == 0,
            "unsupported ocean ruin piece identity or orientation"
        );
        let rotation = match text("Rot")?.as_str() {
            "NONE" => Rotation::None,
            "CLOCKWISE_90" => Rotation::Clockwise90,
            "CLOCKWISE_180" => Rotation::Clockwise180,
            "COUNTERCLOCKWISE_90" => Rotation::Counterclockwise90,
            _ => anyhow::bail!("invalid ocean ruin rotation"),
        };
        let temperature = match text("BiomeType")?.as_str() {
            "WARM" => Temperature::Warm,
            "COLD" => Temperature::Cold,
            _ => anyhow::bail!("invalid ocean ruin temperature"),
        };
        let integrity = match tag.get("Integrity") {
            Some(Tag::Float(value)) => *value,
            _ => anyhow::bail!("invalid ocean ruin integrity"),
        };
        let large = match tag.get("IsLarge") {
            Some(Tag::Byte(0)) => false,
            Some(Tag::Byte(1)) => true,
            _ => anyhow::bail!("invalid ocean ruin large flag"),
        };
        let Some(Tag::IntArray(b)) = tag.get("BB") else {
            anyhow::bail!("invalid ocean ruin bounds")
        };
        ensure!(b.len() == 6, "invalid ocean ruin bounds length");
        let saved = Self {
            selection: Piece {
                template: text("Template")?,
                position: [integer("TPX")?, integer("TPY")?, integer("TPZ")?],
                rotation,
                integrity,
                large,
            },
            temperature,
            bounds: BoundingBox::new([b[0], b[1], b[2]], [b[3], b[4], b[5]])
                .map_err(anyhow::Error::msg)?,
            template_size,
        };
        saved.validate()?;
        Ok(saved)
    }

    /// Restore only onto the same generated piece. Failure leaves it untouched.
    pub fn restore(&self, piece: &mut PreparedPiece, temperature: Temperature) -> Result<()> {
        self.validate()?;
        ensure!(
            self.temperature == temperature
                && self.template_size == piece.template.size
                && self.selection.template == piece.selection.template
                && self.selection.rotation == piece.selection.rotation
                && self.selection.integrity.to_bits() == piece.selection.integrity.to_bits()
                && self.selection.large == piece.selection.large
                && self.selection.position[0] == piece.selection.position[0]
                && self.selection.position[2] == piece.selection.position[2],
            "different ocean ruin piece"
        );
        piece.selection.position = self.selection.position;
        piece.bounds = self.bounds;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::{
        nbt::{self, NamedTag, TagType},
        structure_template::StructureTemplate,
    };
    use std::sync::Arc;

    fn fixtures() -> Result<Vec<(PreparedPiece, SavedOceanRuinPiece)>> {
        let template = Arc::new(StructureTemplate {
            document: NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![]),
            },
            data_version: Some(5023),
            size: [7, 4, 5],
            palettes: vec![],
            blocks: vec![],
            entities: vec![],
        });
        let mut output = vec![];
        for temperature in [Temperature::Warm, Temperature::Cold] {
            for rotation in [
                Rotation::None,
                Rotation::Clockwise90,
                Rotation::Clockwise180,
                Rotation::Counterclockwise90,
            ] {
                for large in [false, true] {
                    for moved in [false, true] {
                        let position = [-37, if moved { 53 } else { 90 }, 52];
                        let piece = PreparedPiece {
                            selection: Piece {
                                template: "minecraft:underwater_ruin/warm_1".into(),
                                position,
                                rotation,
                                integrity: 0.7,
                                large,
                            },
                            template: Arc::clone(&template),
                            bounds: bounding_box(
                                template.size,
                                position,
                                Mirror::None,
                                rotation,
                                [0; 3],
                            )?,
                        };
                        let saved = SavedOceanRuinPiece::capture(&piece, temperature)?;
                        output.push((piece, saved));
                    }
                }
            }
        }
        Ok(output)
    }

    #[test]
    fn strict_round_trip_and_restore() -> Result<()> {
        for (mut piece, saved) in fixtures()? {
            let tag = saved.to_tag()?;
            assert_eq!(SavedOceanRuinPiece::from_tag(&tag, [7, 4, 5])?, saved);
            let original = piece.selection.clone();
            piece.selection.position[1] += 17;
            piece.bounds = bounding_box(
                piece.template.size,
                piece.selection.position,
                Mirror::None,
                piece.selection.rotation,
                [0; 3],
            )?;
            saved.restore(&mut piece, saved.temperature)?;
            assert_eq!(piece.selection, original);
            assert_eq!(piece.bounds, saved.bounds);
            piece.selection.position[0] += 1;
            let before = piece.selection.clone();
            assert!(saved.restore(&mut piece, saved.temperature).is_err());
            assert_eq!(piece.selection, before);
            let Tag::Compound(mut fields) = tag else {
                unreachable!()
            };
            fields.push(("future".into(), Tag::Int(1)));
            assert!(SavedOceanRuinPiece::from_tag(&Tag::Compound(fields), [7, 4, 5]).is_err());
        }
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and a Java 25 JDK"]
    fn saved_piece_nbt_matches_and_round_trips_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        ensure!(manifest["id"] == "26.3", "wrong version");
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"]
            .as_array()
            .context("libraries missing")?
        {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let classpath = std::env::join_paths(paths)?;
        let source = temp.path().join("OceanRuinSavedPieceOracle.java");
        std::fs::write(
            &source,
            include_str!("../../../tests/java/OceanRuinSavedPieceOracle.java"),
        )?;
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compile.status.success(),
            "Java compilation failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let fixtures = fixtures()?;
        let tags = fixtures
            .iter()
            .map(|(_, saved)| saved.to_tag())
            .collect::<Result<Vec<_>>>()?;
        let root = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "pieces".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: tags,
                },
            )]),
        };
        let rust_path = temp.path().join("rust.nbt");
        let java_path = temp.path().join("java.nbt");
        std::fs::write(&rust_path, nbt::write(&root)?)?;
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("OceanRuinSavedPieceOracle")
            .arg(&java_path)
            .arg(&rust_path)
            .output()?;
        ensure!(
            output.status.success(),
            "Java comparison failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let decoded = nbt::read(&std::fs::read(java_path)?)?;
        let Some(Tag::List { values, .. }) = decoded.value.get("pieces") else {
            anyhow::bail!("Java pieces missing")
        };
        ensure!(values.len() == fixtures.len(), "Java piece count differs");
        for (tag, (_, saved)) in values.iter().zip(fixtures) {
            assert_eq!(SavedOceanRuinPiece::from_tag(tag, [7, 4, 5])?, saved);
        }
        Ok(())
    }
}
