//! Prepare ocean-ruin template selections without placing blocks or saving.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use native_persistence::nbt::Tag;
use native_persistence::structure_template::StructureTemplate;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    ocean_ruin::{self, Piece, Settings},
    packs::{ResourceId, ResourceProvider},
};
use serde_json::Value;

pub struct PreparedPiece {
    pub selection: Piece,
    pub template: Arc<StructureTemplate>,
    /// Initial Java template bounds, before ocean-floor postprocessing.
    pub bounds: native_core::beardifier::BoundingBox,
}

/// A structure block is retained for the later data-marker pass and must not
/// be placed as an ordinary template block.
#[derive(Clone, Debug, PartialEq)]
pub enum PreparedPaletteState {
    Block(native_world::loaded::BlockState),
    StructureMarker {
        properties: BTreeMap<String, String>,
    },
}

impl PreparedPiece {
    pub fn block_position(
        &self,
        block: &native_persistence::structure_template::TemplateBlock,
    ) -> [i32; 3] {
        native_resources::structure_transform::block_world_position(
            block.position,
            self.selection.position,
            native_resources::structure_transform::Mirror::None,
            self.selection.rotation,
            [0; 3],
        )
    }

    /// Transform one explicitly selected palette. The block list still uses
    /// NBT order; Java's placement ordering and processors remain separate.
    pub fn transformed_palette(
        &self,
        registry: &GeneratedBlockRegistry,
        palette_index: usize,
    ) -> Result<Vec<PreparedPaletteState>> {
        let palette = self
            .template
            .palettes
            .get(palette_index)
            .context("template palette index out of bounds")?;
        palette
            .iter()
            .map(|tag| {
                let name = match tag.get("id") {
                    Some(Tag::String(name)) => {
                        String::from_utf16(&name.0).context("invalid template block identifier")?
                    }
                    _ => anyhow::bail!("template block identifier missing"),
                };
                let mut properties = BTreeMap::new();
                if let Some(value) = tag.get("properties") {
                    let Tag::Compound(entries) = value else {
                        anyhow::bail!("template properties are not a compound")
                    };
                    for (key, value) in entries {
                        let key =
                            String::from_utf16(&key.0).context("invalid template property name")?;
                        let Tag::String(value) = value else {
                            anyhow::bail!("template property is not a string")
                        };
                        let value = String::from_utf16(&value.0)
                            .context("invalid template property value")?;
                        ensure!(
                            properties.insert(key, value).is_none(),
                            "duplicate template property"
                        );
                    }
                }
                let id = ResourceId::parse(&name)?;
                let name = format!("{}:{}", id.namespace, id.path);
                if name == "minecraft:structure_block" {
                    return Ok(PreparedPaletteState::StructureMarker { properties });
                }
                let input = serde_json::json!({"id": name, "properties": properties});
                let (name, properties) = registry.resolve(&input)?;
                let properties = registry.transform_template_state(
                    &name,
                    &properties,
                    native_resources::structure_transform::Mirror::None,
                    self.selection.rotation,
                )?;
                Ok(PreparedPaletteState::Block(
                    native_world::loaded::BlockState { name, properties },
                ))
            })
            .collect()
    }

    /// Calculate Java's local template load order. This includes structure
    /// blocks; later processors must still filter and handle their markers.
    pub fn block_order(
        &self,
        registry: &GeneratedBlockRegistry,
        palette_index: usize,
    ) -> Result<Vec<usize>> {
        // The supported ocean-ruin states have rotation-invariant static-full
        // classification, verified for all four template rotations.
        let palette = self.transformed_palette(registry, palette_index)?;
        self.template.java_block_order(palette_index, |index, _| {
            let PreparedPaletteState::Block(state) = &palette[index] else {
                anyhow::bail!("structure block without NBT needs collision classification")
            };
            ocean_template_static_full(state)
        })
    }
}

/// Verified collision classes for the vanilla ocean-ruin palette. Unknown
/// data-pack blocks remain errors until their dynamic shape is established.
fn ocean_template_static_full(state: &native_world::loaded::BlockState) -> Result<bool> {
    let id = state
        .name
        .strip_prefix("minecraft:")
        .context("unknown template collision")?;
    if matches!(id, "air" | "cave_air" | "void_air") || id.ends_with("_stairs") {
        return Ok(false);
    }
    if matches!(
        id,
        "bricks"
            | "chiseled_sandstone"
            | "chiseled_stone_bricks"
            | "cobblestone"
            | "cracked_stone_bricks"
            | "cut_sandstone"
            | "dark_oak_planks"
            | "gravel"
            | "light_blue_terracotta"
            | "magma_block"
            | "mossy_cobblestone"
            | "mossy_stone_bricks"
            | "obsidian"
            | "polished_diorite"
            | "polished_granite"
            | "prismarine"
            | "sand"
            | "sandstone"
            | "sea_lantern"
            | "spruce_planks"
            | "stone_bricks"
    ) || id.ends_with("_glazed_terracotta")
    {
        return Ok(true);
    }
    anyhow::bail!(
        "template collision classification is unsupported for {}",
        state.name
    )
}

/// Read the exact selected templates from the same active data-pack stack.
/// Data fixing, processors, entity handling and actual placement remain open.
pub fn prepare_pieces(
    provider: &mut impl ResourceProvider,
    definition: &Value,
    seed: i64,
    chunk: [i32; 2],
) -> Result<Vec<PreparedPiece>> {
    let settings = Settings::decode(definition)?;
    let selected = ocean_ruin::select_for_chunk(seed, chunk, settings)?;
    let mut templates = BTreeMap::<String, Arc<StructureTemplate>>::new();
    let mut result = Vec::with_capacity(selected.len());
    for selection in selected {
        let template = if let Some(template) = templates.get(&selection.template) {
            Arc::clone(template)
        } else {
            let id = ResourceId::parse(&selection.template)?;
            let template = StructureTemplate::load(provider, &id)?
                .with_context(|| format!("missing ocean ruin template {}", selection.template))?;
            ensure!(
                template.data_version == Some(5023),
                "ocean ruin template {} needs data-version conversion",
                selection.template
            );
            let template = Arc::new(template);
            templates.insert(selection.template.clone(), Arc::clone(&template));
            template
        };
        let bounds = native_resources::structure_transform::bounding_box(
            template.size,
            selection.position,
            native_resources::structure_transform::Mirror::None,
            selection.rotation,
            [0; 3],
        )?;
        result.push(PreparedPiece {
            selection,
            template,
            bounds,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::nbt::{self, NamedTag, Tag, TagType};
    use serde_json::json;
    use std::io::Write;

    struct Data {
        bytes: Option<Vec<u8>>,
        reads: BTreeMap<String, usize>,
    }
    impl ResourceProvider for Data {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            *self.reads.entry(path.to_owned()).or_default() += 1;
            Ok(self.bytes.clone())
        }
    }
    fn template(version: i32) -> Vec<u8> {
        let list = |kind, values| Tag::List {
            element_type: kind,
            values,
        };
        let doc = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(version)),
                (
                    "size".into(),
                    list(TagType::Int, vec![Tag::Int(1), Tag::Int(1), Tag::Int(1)]),
                ),
                (
                    "palette".into(),
                    list(TagType::Compound, vec![Tag::Compound(vec![])]),
                ),
                ("blocks".into(), list(TagType::Compound, vec![])),
                ("entities".into(), list(TagType::Compound, vec![])),
            ]),
        };
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&nbt::write(&doc).unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn prepares_active_templates_once_and_rejects_missing_or_old_data() {
        let definition = json!({"type":"minecraft:ocean_ruin","biome_temp":"cold","large_probability":1.0,"cluster_probability":1.0});
        let mut data = Data {
            bytes: Some(template(5023)),
            reads: BTreeMap::new(),
        };
        let prepared = prepare_pieces(&mut data, &definition, 42, [0, 0]).unwrap();
        assert!(!prepared.is_empty());
        assert!(data.reads.values().all(|&count| count == 1));
        for (index, piece) in prepared.iter().enumerate() {
            for other in &prepared[..index] {
                if other.selection.template == piece.selection.template {
                    assert!(Arc::ptr_eq(&other.template, &piece.template));
                }
            }
        }
        data.bytes = Some(template(4000));
        assert!(
            format!(
                "{:#}",
                prepare_pieces(&mut data, &definition, 42, [0, 0])
                    .err()
                    .unwrap()
            )
            .contains("data-version conversion")
        );
        data.bytes = None;
        assert!(
            format!(
                "{:#}",
                prepare_pieces(&mut data, &definition, 42, [0, 0])
                    .err()
                    .unwrap()
            )
            .contains("missing ocean ruin template")
        );
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn prepares_selected_vanilla_ocean_ruin_templates() -> Result<()> {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut data = native_resources::packs::ResourceStack::open(&game)?;
        let registry = data.vanilla_generated_block_registry()?;
        let mut markers = 0;
        let mut glazed = 0;
        for temperature in ["warm", "cold"] {
            let definition = json!({"type":"minecraft:ocean_ruin","biome_temp":temperature,"large_probability":0.5,"cluster_probability":0.9});
            for seed in 0..16 {
                let pieces = prepare_pieces(&mut data, &definition, seed, [-3, 2])?;
                assert!(!pieces.is_empty());
                assert!(pieces.iter().all(|piece| !piece.template.blocks.is_empty()));
                for piece in &pieces {
                    for index in 0..piece.template.palettes.len() {
                        let palette = piece.transformed_palette(&registry, index)?;
                        assert_eq!(palette.len(), piece.template.palettes[index].len());
                        for state in palette {
                            match state {
                                PreparedPaletteState::StructureMarker { .. } => markers += 1,
                                PreparedPaletteState::Block(state)
                                    if state.name.ends_with("_glazed_terracotta") =>
                                {
                                    glazed += 1
                                }
                                _ => {}
                            }
                        }
                    }
                    for block in &piece.template.blocks {
                        let pos = piece.block_position(block);
                        assert!((0..3).all(|axis| {
                            (piece.bounds.min[axis]..=piece.bounds.max[axis]).contains(&pos[axis])
                        }));
                    }
                }
            }
        }
        assert!(markers > 0);
        assert!(glazed > 0);
        Ok(())
    }

    #[test]
    #[ignore = "requires Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn vanilla_ruin_template_order_matches_java() -> Result<()> {
        use native_resources::structure_transform::Rotation;
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
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
        let source = temp.path().join("OceanRuinTemplateOrderOracle.java");
        std::fs::write(
            &source,
            include_str!("../../tests/java/OceanRuinTemplateOrderOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let mut names = Vec::new();
        for prefix in ["brick", "cracked", "mossy", "warm"] {
            for n in 1..=8 {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for prefix in ["big_brick", "big_cracked", "big_mossy"] {
            for n in [1, 2, 3, 8] {
                names.push(format!("underwater_ruin/{prefix}_{n}"));
            }
        }
        for n in 4..=7 {
            names.push(format!("underwater_ruin/big_warm_{n}"));
        }
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(classpath)
            .arg("OceanRuinTemplateOrderOracle")
            .args(&names)
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let output = String::from_utf8(output.stdout)?;
        let mut rows = output
            .lines()
            .filter(|line| line.starts_with("T\t") || line.starts_with("B\t"));
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let mut total = 0;
        for name in &names {
            let header = rows.next().context("missing Java template")?;
            let mut fields = header.split('\t');
            ensure!(
                fields.next() == Some("T") && fields.next() == Some(name.as_str()),
                "unexpected Java template {header}"
            );
            let expected_count: usize = fields.next().context("missing Java count")?.parse()?;
            let template = Arc::new(
                StructureTemplate::load(&mut resources, &ResourceId::parse(name)?)?
                    .context("missing template")?,
            );
            let bounds = native_resources::structure_transform::bounding_box(
                template.size,
                [0, 90, 0],
                native_resources::structure_transform::Mirror::None,
                Rotation::None,
                [0; 3],
            )?;
            let mut piece = PreparedPiece {
                selection: Piece {
                    template: format!("minecraft:{name}"),
                    position: [0, 90, 0],
                    rotation: Rotation::None,
                    integrity: 0.8,
                    large: false,
                },
                template,
                bounds,
            };
            let palette = piece.transformed_palette(&registry, 0)?;
            let order = piece.block_order(&registry, 0)?;
            for rotation in [
                Rotation::Clockwise90,
                Rotation::Clockwise180,
                Rotation::Counterclockwise90,
            ] {
                piece.selection.rotation = rotation;
                assert_eq!(
                    piece.block_order(&registry, 0)?,
                    order,
                    "rotation changed load order for {name}"
                );
            }
            piece.selection.rotation = Rotation::None;
            assert_eq!(order.len(), expected_count, "{name}");
            for index in order {
                let row: Value = serde_json::from_str(
                    rows.next()
                        .context("missing Java block")?
                        .strip_prefix("B\t")
                        .context("unexpected Java row")?,
                )?;
                let block = &piece.template.blocks[index];
                assert_eq!(
                    block.position,
                    [
                        row["x"].as_i64().unwrap() as i32,
                        row["y"].as_i64().unwrap() as i32,
                        row["z"].as_i64().unwrap() as i32
                    ],
                    "{name}"
                );
                assert_eq!(block.nbt.is_some(), row["nbt"].as_bool().unwrap(), "{name}");
                let (id, properties) = match &palette[block.state] {
                    PreparedPaletteState::Block(state) => (state.name.as_str(), &state.properties),
                    PreparedPaletteState::StructureMarker { properties } => {
                        ("minecraft:structure_block", properties)
                    }
                };
                assert_eq!(id, row["id"].as_str().unwrap(), "{name}");
                assert_eq!(
                    serde_json::to_value(properties)?,
                    row["properties"],
                    "{name}"
                );
                total += 1;
            }
        }
        assert!(rows.next().is_none());
        println!(
            "verified {total} ordered template blocks across {} ruins",
            names.len()
        );
        Ok(())
    }
}
