//! Prepare an existing world's noise generator from captured settings.
use super::*;
use anyhow::{Context, bail};
use native_persistence::{
    nbt::{NamedTag, Tag},
    world::{DimensionId, GenerationContextSnapshot, GenerationInputSnapshot},
};

/// A generator bound to the exact captured inputs from which it was prepared.
/// Live state, data-pack changes and generation completeness still need their
/// own checks before any generated data may be saved.
pub(crate) struct PreparedExistingGenerator {
    pub generator: TerrainGenerator,
    pub lighting: GeneratedDimensionLighting,
    pub dimension_definition: Value,
    pub generate_structures: bool,
    pub resources: ResourceStack,
    context: GenerationContextSnapshot,
}

impl PreparedExistingGenerator {
    pub fn inputs(&self) -> &GenerationInputSnapshot {
        self.context.inputs()
    }

    /// Bind captured disk and pack inputs; live session state and generation
    /// completeness still require independent checks before saving.
    pub fn add_save_dependencies(
        &self,
        plan: &mut native_persistence::save::WorldSavePlan,
    ) -> Result<()> {
        plan.add_generation_context(&self.context)
    }
}

fn numeric_bool(tag: &Tag) -> Option<bool> {
    Some(match tag {
        Tag::Byte(v) => *v != 0,
        Tag::Short(v) => *v != 0,
        Tag::Int(v) => *v != 0,
        Tag::Long(v) => *v != 0,
        Tag::Float(v) if v.is_finite() => *v != 0.0,
        Tag::Double(v) if v.is_finite() => *v != 0.0,
        _ => return None,
    })
}

// NBT encodes booleans as numbers, so only fields known to have boolean
// codec types can be converted without changing ordinary numeric values.
fn boolean_key(key: &str) -> bool {
    matches!(
        key,
        "has_skylight"
            | "has_ceiling"
            | "has_fixed_time"
            | "has_ender_dragon_fight"
            | "destroy_on_use"
            | "destroy_on_leave"
            | "gameplay/fast_lava"
            | "minecraft:gameplay/fast_lava"
            | "gameplay/water_evaporates"
            | "minecraft:gameplay/water_evaporates"
            | "gameplay/respawn_anchor_works"
            | "minecraft:gameplay/respawn_anchor_works"
            | "legacy_random_source"
            | "disable_mob_generation"
            | "aquifers_enabled"
            | "ore_veins_enabled"
            | "is_3d"
            | "add_stone_depth"
            | "add_surface_depth"
    )
}

fn saved_value(value: &Tag, boolean: bool, depth: usize, nodes: &mut usize) -> Result<Value> {
    ensure!(depth <= 128, "saved dimension is too deeply nested");
    *nodes = nodes
        .checked_add(1)
        .context("saved dimension node count overflow")?;
    ensure!(*nodes <= 1_000_000, "saved dimension has too many values");
    if boolean && let Some(value) = numeric_bool(value) {
        return Ok(Value::Bool(value));
    }
    Ok(match value {
        Tag::Byte(v) => Value::from(*v),
        Tag::Short(v) => Value::from(*v),
        Tag::Int(v) => Value::from(*v),
        Tag::Long(v) => Value::from(*v),
        Tag::Float(v) => Value::Number(
            serde_json::Number::from_f64(f64::from(*v)).context("non-finite saved value")?,
        ),
        Tag::Double(v) => {
            Value::Number(serde_json::Number::from_f64(*v).context("non-finite saved value")?)
        }
        Tag::String(v) => Value::String(String::from_utf16(&v.0).context("invalid saved string")?),
        Tag::List { values, .. } => Value::Array(
            values
                .iter()
                .map(|v| saved_value(v, false, depth + 1, nodes))
                .collect::<Result<_>>()?,
        ),
        Tag::Compound(fields) => {
            let mut output = serde_json::Map::new();
            for (key, value) in fields {
                let key = String::from_utf16(&key.0).context("invalid saved key")?;
                let is_boolean = boolean_key(&key) || (boolean && key == "argument");
                let value = saved_value(value, is_boolean, depth + 1, nodes)?;
                ensure!(
                    output.insert(key, value).is_none(),
                    "duplicate saved dimension field"
                );
            }
            Value::Object(output)
        }
        Tag::ByteArray(values) => {
            Value::Array(values.iter().map(|v| Value::from(*v as i8)).collect())
        }
        Tag::IntArray(values) => Value::Array(values.iter().map(|v| Value::from(*v)).collect()),
        Tag::LongArray(values) => Value::Array(values.iter().map(|v| Value::from(*v)).collect()),
    })
}

fn resolve_stem(
    provider: &mut impl ResourceProvider,
    document: &NamedTag,
    dimension: &DimensionId,
) -> Result<(Value, bool)> {
    let Tag::Compound(data) = document
        .value
        .get("data")
        .context("missing world generation data")?
    else {
        bail!("world generation data is not a compound");
    };
    let one = |name: &str| -> Result<Option<&Tag>> {
        let mut found = data.iter().filter(|(key, _)| key.matches(name));
        let value = found.next().map(|(_, value)| value);
        ensure!(
            found.next().is_none(),
            "duplicate world generation field {name}"
        );
        Ok(value)
    };
    let generate_structures = one("generate_structures")?
        .map(|v| numeric_bool(v).context("invalid generate_structures"))
        .transpose()?
        .unwrap_or(true);
    let Tag::Compound(dimensions) = one("dimensions")?.context("missing saved dimensions")? else {
        bail!("saved dimensions are not a compound");
    };
    let mut saved = None;
    let mut names = BTreeSet::new();
    for (name, stem) in dimensions {
        let name = String::from_utf16(&name.0).context("invalid saved dimension name")?;
        let id = DimensionId::parse(&name)?;
        ensure!(names.insert(id.clone()), "duplicate saved dimension {name}");
        if &id == dimension {
            saved = Some(stem);
        }
    }
    let path = format!(
        "data/{}/dimension/{}.json",
        dimension.namespace, dimension.path
    );
    if let Some(bytes) = provider.read_data(&path)? {
        ensure!(
            bytes.len() <= 16 * 1024 * 1024,
            "dimension definition is too large"
        );
        let stem =
            serde_json::from_slice(&bytes).with_context(|| format!("invalid dimension {path}"))?;
        return Ok((stem, generate_structures));
    }
    let saved = saved.context("dimension is missing from the saved world and active packs")?;
    ensure!(
        matches!(saved, Tag::Compound(_)),
        "saved dimension is not a compound"
    );
    Ok((saved_value(saved, false, 0, &mut 0)?, generate_structures))
}

/// Compile using the saved generator and the pack selection captured from the
/// same world. The pack source still needs protection before a later save.
pub(crate) fn prepare_existing_generator(
    context: GenerationContextSnapshot,
    minecraft_dir: &std::path::Path,
) -> Result<PreparedExistingGenerator> {
    let packs = Arc::clone(context.packs());
    packs.verify()?;
    let selected = context.selection();
    let selection = DataPackSettings {
        enabled: selected.enabled.clone(),
        disabled: selected.disabled.clone(),
        enabled_features: selected.enabled_features.clone(),
    };
    let mut resources = ResourceStack::open(minecraft_dir)?;
    let warnings = resources.configure_snapshot_data(Arc::clone(&packs), &selection)?;
    ensure!(
        warnings.is_empty(),
        "data pack selection needs attention: {}",
        warnings.join("; ")
    );
    let inputs = context.inputs();
    let dimension = inputs.sources().dimension();
    let (stem, generate_structures) =
        resolve_stem(&mut resources, inputs.settings().document(), dimension)?;
    let id = ResourceId::parse(&format!("{}:{}", dimension.namespace, dimension.path))?;
    let (lighting, definition, dimension_definition) =
        GeneratedDimensionLighting::from_stem(&mut resources, id, &stem)?;
    let seed = inputs.settings().seed();
    let generator = if generate_structures {
        TerrainGenerator::with_structure_context(
            &mut resources,
            &definition,
            seed,
            lighting.min_y,
            lighting.height,
        )?
    } else {
        TerrainGenerator::without_terrain_adjustments(
            &mut resources,
            &definition,
            seed,
            lighting.min_y,
            lighting.height,
        )?
    };
    packs.verify()?;
    Ok(PreparedExistingGenerator {
        generator,
        lighting,
        dimension_definition,
        generate_structures,
        resources,
        context,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::nbt::{JavaString, TagType};
    use serde_json::json;

    struct Packs(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Packs {
        fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }

    fn settings(stem: Tag) -> NamedTag {
        NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![
                ("DataVersion".into(), Tag::Int(5023)),
                (
                    "data".into(),
                    Tag::Compound(vec![
                        ("seed".into(), Tag::Long(42)),
                        ("generate_structures".into(), Tag::Byte(0)),
                        (
                            "dimensions".into(),
                            Tag::Compound(vec![("minecraft:overworld".into(), stem)]),
                        ),
                    ]),
                ),
            ]),
        }
    }

    #[test]
    fn saved_stems_keep_numeric_values_and_respect_active_overrides() -> Result<()> {
        let stem = Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:overworld".into())),
            (
                "generator".into(),
                Tag::Compound(vec![
                    ("type".into(), Tag::String("minecraft:noise".into())),
                    (
                        "settings".into(),
                        Tag::Compound(vec![
                            ("legacy_random_source".into(), Tag::Byte(1)),
                            ("sea_level".into(), Tag::Byte(1)),
                            (
                                "material_rule".into(),
                                Tag::Compound(vec![("add_stone_depth".into(), Tag::Byte(0))]),
                            ),
                        ]),
                    ),
                    (
                        "biome_source".into(),
                        Tag::Compound(vec![
                            ("type".into(), Tag::String("minecraft:fixed".into())),
                            ("biome".into(), Tag::String("minecraft:plains".into())),
                        ]),
                    ),
                ]),
            ),
        ]);
        let doc = settings(stem.clone());
        let dimension = DimensionId::parse("overworld")?;
        let mut packs = Packs(BTreeMap::new());
        let (saved, structures) = resolve_stem(&mut packs, &doc, &dimension)?;
        assert!(!structures);
        assert_eq!(saved["generator"]["settings"]["legacy_random_source"], true);
        assert_eq!(saved["generator"]["settings"]["sea_level"], 1);
        assert_eq!(
            saved["generator"]["settings"]["material_rule"]["add_stone_depth"],
            false
        );
        let active = json!({"type":"minecraft:the_nether","generator":{"type":"minecraft:noise","settings":"minecraft:nether","biome_source":{"type":"minecraft:the_end"}}});
        packs.0.insert(
            "data/minecraft/dimension/overworld.json".into(),
            serde_json::to_vec(&active)?,
        );
        assert_eq!(resolve_stem(&mut packs, &doc, &dimension)?.0, active);
        assert!(resolve_stem(&mut packs, &doc, &DimensionId::parse("minecraft:missing")?).is_err());
        let duplicate = settings(Tag::Compound(vec![
            ("type".into(), Tag::String("minecraft:overworld".into())),
            ("type".into(), Tag::String("minecraft:the_nether".into())),
        ]));
        packs.0.clear();
        assert!(resolve_stem(&mut packs, &duplicate, &dimension).is_err());
        let malformed = settings(Tag::Compound(vec![(
            "type".into(),
            Tag::String(JavaString(vec![0xd800])),
        )]));
        assert!(resolve_stem(&mut packs, &malformed, &dimension).is_err());
        assert!(
            saved_value(
                &Tag::List {
                    element_type: TagType::Int,
                    values: vec![Tag::Int(7)]
                },
                false,
                0,
                &mut 0
            )?
            .is_array()
        );
        Ok(())
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and a temporary world"]
    fn captured_vanilla_world_prepares_each_noise_dimension() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_persistence::{
            save::WorldSavePlan,
            world::{ChunkPos, WorldReader},
            worldgen_settings::{NewWorldOptions, draft_baked_worldgen_settings},
        };
        use std::io::Write;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let mut source = ResourceStack::open(&game)?;
        let worldgen = draft_baked_worldgen_settings(
            &mut source,
            "minecraft:normal",
            NewWorldOptions {
                seed: -42,
                generate_structures: true,
                bonus_chest: false,
            },
        )?;
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&worldgen)?,
        )?;
        let level = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![(
                "Data".into(),
                Tag::Compound(vec![("DataVersion".into(), Tag::Int(5023))]),
            )]),
        };
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        for name in ["overworld", "the_nether", "the_end"] {
            std::fs::create_dir_all(root.join(format!("dimensions/minecraft/{name}/region")))?;
            let dimension = DimensionId::parse(name)?;
            let mut reader = WorldReader::open_for_session(root, dimension)?;
            let context = reader.capture_generation_context([ChunkPos { x: 0, z: 0 }])?;
            let mut actual = prepare_existing_generator(context, &game)?;
            assert!(actual.generate_structures);
            assert_eq!(actual.inputs().settings().seed(), -42);
            let mut plan = WorldSavePlan::new();
            actual.add_save_dependencies(&mut plan)?;
            assert!(actual.add_save_dependencies(&mut plan).is_err());
            let mut reference_resources = ResourceStack::open(&game)?;
            let (lighting, definition, dimension_definition) =
                GeneratedDimensionLighting::load_preset_with_definition(
                    &mut reference_resources,
                    "minecraft:normal",
                    &format!("minecraft:{name}"),
                )?;
            assert_eq!(actual.lighting, lighting);
            assert_eq!(actual.dimension_definition, dimension_definition);
            let mut reference = TerrainGenerator::with_structure_context(
                &mut reference_resources,
                &definition,
                -42,
                lighting.min_y,
                lighting.height,
            )?;
            assert_eq!(
                actual.generator.sea_level(),
                reference.sea_level(),
                "{name}"
            );
            assert_eq!(
                actual.generator.possible_biomes(),
                reference.possible_biomes(),
                "{name}"
            );
            let mut a = native_resources::density::DensityEvaluator::default();
            let mut b = native_resources::density::DensityEvaluator::default();
            assert_eq!(
                actual.generator.generate_biomes([0, 0], &mut a)?.values(),
                reference.generate_biomes([0, 0], &mut b)?.values(),
                "{name}"
            );
            assert!(actual.resources.data_pack_snapshot().is_some());
        }
        let no_structures = draft_baked_worldgen_settings(
            &mut source,
            "minecraft:normal",
            NewWorldOptions {
                seed: -42,
                generate_structures: false,
                bonus_chest: false,
            },
        )?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&no_structures)?,
        )?;
        let pack = root.join("datapacks/custom");
        let dimensions = pack.join("data/minecraft/dimension");
        std::fs::create_dir_all(&dimensions)?;
        std::fs::write(
            pack.join("pack.mcmeta"),
            br#"{"pack":{"description":"test","min_format":121,"max_format":121}}"#,
        )?;
        let dimension_file = dimensions.join("overworld.json");
        let custom_stem = |biome: &str| {
            json!({
                "type":"minecraft:overworld",
                "generator":{"type":"minecraft:noise","settings":"minecraft:overworld",
                    "biome_source":{"type":"minecraft:fixed","biome":biome}}
            })
        };
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&custom_stem("minecraft:desert"))?,
        )?;
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let mut updated_level = level.clone();
        let Tag::Compound(root_fields) = &mut updated_level.value else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut root_fields
            .iter_mut()
            .find(|(name, _)| name.matches("Data"))
            .unwrap()
            .1
        else {
            unreachable!()
        };
        data.push((
            "DataPacks".into(),
            Tag::Compound(vec![
                (
                    "Enabled".into(),
                    Tag::List {
                        element_type: TagType::String,
                        values: vec![
                            Tag::String("vanilla".into()),
                            Tag::String("file/custom".into()),
                        ],
                    },
                ),
                (
                    "Disabled".into(),
                    Tag::List {
                        element_type: TagType::End,
                        values: vec![],
                    },
                ),
            ]),
        ));
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&native_persistence::nbt::write(&updated_level)?)?;
        std::fs::write(root.join("level.dat"), gzip.finish()?)?;
        let context = reader.capture_generation_context([ChunkPos { x: 0, z: 0 }])?;
        assert_eq!(context.selection().enabled, ["vanilla", "file/custom"]);
        std::fs::write(
            &dimension_file,
            serde_json::to_vec(&custom_stem("minecraft:plains"))?,
        )?;
        let actual = prepare_existing_generator(context, &game)?;
        assert!(!actual.generate_structures);
        assert_eq!(
            actual.generator.possible_biomes(),
            &BTreeSet::from([ResourceId::parse("minecraft:desert")?])
        );
        Ok(())
    }
}
