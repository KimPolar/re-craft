//! Prepare an existing world's noise generator from captured settings.
use super::*;
use anyhow::{Context, bail};
use native_persistence::{
    nbt::{NamedTag, Tag},
    world::{DimensionId, GenerationContextSnapshot, GenerationInputSnapshot, WorldReader},
};
use native_session::worldgen::schedule::GenerationSchedule;

/// A generator bound to the exact captured inputs from which it was prepared.
/// Bind the captured inputs with `add_save_dependencies` when saving.
/// Live state and generation completeness still need independent checks.
pub(crate) struct PreparedExistingGenerator {
    pub generator: TerrainGenerator,
    pub lighting: GeneratedDimensionLighting,
    pub dimension_definition: Value,
    pub generate_structures: bool,
    pub resources: ResourceStack,
    context: GenerationContextSnapshot,
}

/// Read-only restoration of captured disk inputs. It does not include unsaved
/// session changes or prove that the chunks can resume every generation stage.
pub(crate) struct RestoredSavedTerrain {
    pub world: native_world::loaded::World,
    pub biomes:
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>,
    pub heightmaps: BTreeMap<
        native_world::loaded::ChunkPos,
        native_persistence::heightmap::SavedGenerationHeightmaps,
    >,
    pub postprocessing: BTreeMap<
        native_world::loaded::ChunkPos,
        Option<native_core::postprocessing::PostProcessing>,
    >,
}

impl PreparedExistingGenerator {
    pub fn inputs(&self) -> &GenerationInputSnapshot {
        self.context.inputs()
    }

    /// Restore captured biome volumes against this generator's pack registry.
    /// The caller must still restore other stage data before advancing a plan.
    pub fn restore_saved_biomes(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<
        BTreeMap<native_world::loaded::ChunkPos, Arc<native_session::worldgen::QuartBiomeChunk>>,
    > {
        let registry: BTreeSet<_> = self
            .resources
            .list_data("worldgen/biome")?
            .into_iter()
            .collect();
        ensure!(!registry.is_empty(), "active biome registry is empty");
        let mut restored = BTreeMap::new();
        let sources = self.context.inputs().sources().chunks();
        for position in positions {
            let source = sources
                .get(&position)
                .context("biome chunk was not captured")?;
            let source = source
                .terrain
                .as_ref()
                .context("saved biome chunk is missing")?;
            let chunk = native_session::worldgen::QuartBiomeChunk::from_saved_chunk(
                source,
                self.context.inputs().settings().seed(),
                self.lighting.min_y,
                self.lighting.height,
                |biome| {
                    ensure!(
                        registry.contains(biome),
                        "saved biome is not in the active registry: {}:{}",
                        biome.namespace,
                        biome.path
                    );
                    Ok(())
                },
            )?;
            let position = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            ensure!(
                restored.insert(position, Arc::new(chunk)).is_none(),
                "duplicate saved biome chunk"
            );
        }
        ensure!(!restored.is_empty(), "no saved biome chunks requested");
        Ok(restored)
    }

    /// Restore captured terrain and recorded generation metadata without
    /// replacing unknown blocks, priming heightmaps or consuming marks.
    pub fn restore_saved_terrain(
        &mut self,
        positions: impl IntoIterator<Item = native_persistence::world::ChunkPos>,
    ) -> Result<RestoredSavedTerrain> {
        use native_persistence::{
            heightmap::{HeightBounds, SavedGenerationHeightmaps},
            world::ChunkArea,
        };
        use native_session::worldgen::schedule::GenerationStage;

        let mut requested = BTreeSet::new();
        for position in positions {
            ensure!(requested.insert(position), "duplicate saved terrain chunk");
        }
        ensure!(!requested.is_empty(), "no saved terrain chunks requested");
        let biomes = self.restore_saved_biomes(requested.iter().copied())?;
        let block_registry = self.resources.vanilla_generated_block_registry()?;
        let bounds = HeightBounds {
            min_y: self.lighting.min_y,
            max_y: self
                .lighting
                .min_y
                .checked_add(self.lighting.height)
                .context("dimension height overflow")?,
        };
        let mut chunks = BTreeMap::new();
        let mut heightmaps = BTreeMap::new();
        let mut postprocessing = BTreeMap::new();
        for position in requested {
            let source = self
                .context
                .inputs()
                .sources()
                .chunks()
                .get(&position)
                .context("terrain chunk was not captured")?
                .terrain
                .as_ref()
                .context("saved terrain chunk is missing")?;
            let status = source
                .status
                .as_ref()
                .context("saved chunk status is missing")?;
            let status = String::from_utf16(&status.0).context("invalid saved chunk status")?;
            ensure!(
                GenerationStage::from_id(&status)
                    .is_some_and(|stage| stage >= GenerationStage::Terrain),
                "saved chunk has not generated terrain"
            );
            let key = native_world::loaded::ChunkPos {
                x: position.x,
                z: position.z,
            };
            heightmaps.insert(
                key,
                SavedGenerationHeightmaps::read(&source.document, bounds)?,
            );
            let Tag::Compound(fields) = &source.document.value else {
                bail!("saved chunk is not a compound")
            };
            let mut marks = fields
                .iter()
                .filter(|(name, _)| name.matches("PostProcessing"));
            let decoded = marks
                .next()
                .map(|(_, tag)| {
                    native_persistence::postprocessing::decode(
                        tag,
                        bounds.min_y,
                        self.lighting.height,
                    )
                })
                .transpose()?;
            ensure!(
                marks.next().is_none(),
                "duplicate saved postprocessing field"
            );
            postprocessing.insert(key, decoded);
            chunks.insert(position, Arc::new(source.clone()));
        }
        let area = ChunkArea {
            center: *chunks.keys().next().expect("nonempty requested set"),
            radius: 0,
            chunks,
            missing: vec![],
        };
        let world = area.materialize_with_defaults(
            self.context.inputs().sources().dimension(),
            &block_registry.default_states(),
        )?;
        for (_, state) in world.states() {
            ensure!(
                block_registry.is_canonical_state(&state.name, &state.properties),
                "unsupported saved block state: {}",
                state.name
            );
        }
        Ok(RestoredSavedTerrain {
            world,
            biomes,
            heightmaps,
            postprocessing,
        })
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
/// same world. A later save must bind these inputs as transaction dependencies.
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

/// Capture the scheduler's complete current neighborhood before preparing the
/// generator. If the schedule is expanded, callers must capture and prepare
/// again; dynamic inputs outside this schedule and unsaved session changes
/// still need independent tracking.
pub(crate) fn prepare_existing_generator_for_schedule(
    reader: &mut WorldReader,
    schedule: &GenerationSchedule,
    minecraft_dir: &std::path::Path,
) -> Result<PreparedExistingGenerator> {
    let context = capture_scheduled_context(reader, schedule)?;
    prepare_existing_generator(context, minecraft_dir)
}

fn capture_scheduled_context(
    reader: &mut WorldReader,
    schedule: &GenerationSchedule,
) -> Result<GenerationContextSnapshot> {
    ensure!(schedule.requested_chunks() != 0, "no generation requests");
    reader.capture_generation_context(
        schedule
            .requirements()
            .map(|(pos, _)| native_persistence::world::ChunkPos { x: pos.x, z: pos.z }),
    )
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
    fn scheduled_capture_tracks_every_required_neighbor_and_expansion() -> Result<()> {
        use flate2::{Compression, write::GzEncoder};
        use native_session::worldgen::schedule::GenerationStage;
        use native_world::loaded::ChunkPos;
        use std::io::Write;

        let world = tempfile::tempdir()?;
        let root = world.path();
        std::fs::create_dir_all(root.join("data/minecraft"))?;
        std::fs::create_dir_all(root.join("dimensions/minecraft/overworld/region"))?;
        std::fs::write(
            root.join("data/minecraft/world_gen_settings.dat"),
            native_persistence::nbt::write(&settings(Tag::Compound(vec![])))?,
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
        let mut reader = WorldReader::open_for_session(root, DimensionId::parse("overworld")?)?;
        let mut schedule = GenerationSchedule::new();
        assert!(capture_scheduled_context(&mut reader, &schedule).is_err());
        let target = ChunkPos { x: -3, z: 4 };
        schedule.request_many([
            (target, GenerationStage::Full),
            (ChunkPos { x: -2, z: 4 }, GenerationStage::Terrain),
        ])?;
        let expected: BTreeSet<_> = schedule
            .requirements()
            .map(|(pos, _)| (pos.x, pos.z))
            .collect();
        let captured = capture_scheduled_context(&mut reader, &schedule)?;
        let sources = captured.inputs().sources().chunks();
        assert_eq!(
            sources
                .keys()
                .map(|pos| (pos.x, pos.z))
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert!(sources.values().all(|source| {
            source.terrain.is_none() && source.entities.is_none() && source.poi.is_none()
        }));
        assert!(expected.len() > 1);

        let before = schedule.requested_chunks();
        assert!(
            schedule
                .request(ChunkPos { x: i32::MAX, z: 0 }, GenerationStage::Full)
                .is_err()
        );
        assert_eq!(schedule.requested_chunks(), before);
        schedule.request(ChunkPos { x: 50, z: 50 }, GenerationStage::Full)?;
        let expanded = capture_scheduled_context(&mut reader, &schedule)?;
        assert!(expanded.inputs().sources().chunks().len() > sources.len());
        assert_eq!(
            expanded.inputs().sources().chunks().len(),
            schedule.requested_chunks()
        );
        Ok(())
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
            use native_persistence::save::{FreshRegionDraft, FreshRegionKind};
            let region_dir = root.join(format!("dimensions/minecraft/{name}/region"));
            std::fs::create_dir_all(&region_dir)?;
            let lighting = GeneratedDimensionLighting::load_preset(
                &mut source,
                "minecraft:normal",
                &format!("minecraft:{name}"),
            )?;
            let sections = (0..lighting.height / 16)
                .map(|index| {
                    let section_y = i8::try_from(lighting.min_y / 16 + index).unwrap();
                    let biome = if index % 2 == 0 {
                        "minecraft:plains"
                    } else {
                        "minecraft:forest"
                    };
                    Tag::Compound(vec![
                        ("Y".into(), Tag::Byte(section_y)),
                        (
                            "block_states".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String("minecraft:stone".into())],
                                },
                            )]),
                        ),
                        (
                            "biomes".into(),
                            Tag::Compound(vec![(
                                "palette".into(),
                                Tag::List {
                                    element_type: TagType::String,
                                    values: vec![Tag::String(biome.into())],
                                },
                            )]),
                        ),
                    ])
                })
                .collect();
            let saved_height = [lighting.min_y + 7; 256];
            let maps = native_persistence::heightmap::encode_generated_heightmaps(
                native_persistence::heightmap::HeightBounds {
                    min_y: lighting.min_y,
                    max_y: lighting.min_y + lighting.height,
                },
                &native_persistence::heightmap::GeneratedHeightmaps {
                    world_surface_wg: Some(&saved_height),
                    ..Default::default()
                },
            )?;
            let mut marks =
                native_core::postprocessing::PostProcessing::new(lighting.min_y, lighting.height)
                    .unwrap();
            marks.mark(0, lighting.min_y + 1, 0);
            marks.mark(0, lighting.min_y + 1, 0);
            let saved = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(0)),
                    ("zPos".into(), Tag::Int(0)),
                    ("Status".into(), Tag::String("minecraft:full".into())),
                    ("Heightmaps".into(), maps),
                    (
                        "PostProcessing".into(),
                        native_persistence::postprocessing::encode(&marks),
                    ),
                    (
                        "sections".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: sections,
                        },
                    ),
                ]),
            };
            let mut region = FreshRegionDraft::new(FreshRegionKind::Terrain, 0, 0);
            region.add(ChunkPos { x: 0, z: 0 }, &saved, 1)?;
            let mut unknown = saved.clone();
            let Tag::Compound(fields) = &mut unknown.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|(key, _)| key.matches("xPos"))
                .unwrap()
                .1 = Tag::Int(1);
            let Tag::List {
                values: sections, ..
            } = &mut fields
                .iter_mut()
                .find(|(key, _)| key.matches("sections"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            let Tag::Compound(section) = &mut sections[0] else {
                unreachable!()
            };
            let Tag::Compound(states) = &mut section
                .iter_mut()
                .find(|(key, _)| key.matches("block_states"))
                .unwrap()
                .1
            else {
                unreachable!()
            };
            states[0].1 = Tag::List {
                element_type: TagType::String,
                values: vec![Tag::String("example:unknown".into())],
            };
            region.add(ChunkPos { x: 1, z: 0 }, &unknown, 1)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .open(region_dir.join("r.0.0.mca"))?;
            region.write_staged(&mut file)?;
            drop(file);
            let dimension = DimensionId::parse(name)?;
            let mut reader = WorldReader::open_for_session(root, dimension)?;
            let mut schedule = GenerationSchedule::new();
            schedule.request(
                native_world::loaded::ChunkPos { x: 0, z: 0 },
                native_session::worldgen::schedule::GenerationStage::Full,
            )?;
            let mut actual =
                prepare_existing_generator_for_schedule(&mut reader, &schedule, &game)?;
            assert_eq!(
                actual.inputs().sources().chunks().len(),
                schedule.requested_chunks()
            );
            assert!(actual.generate_structures);
            assert_eq!(actual.inputs().settings().seed(), -42);
            let position = ChunkPos { x: 0, z: 0 };
            let restored = actual.restore_saved_biomes([position])?;
            let chunk = &restored[&native_world::loaded::ChunkPos { x: 0, z: 0 }];
            let mut neighborhood = actual.generator.biome_neighborhood()?;
            neighborhood.insert(Arc::clone(chunk))?;
            assert_eq!(
                neighborhood
                    .noise_biome_at_block(0, lighting.min_y, 0)?
                    .path,
                "plains"
            );
            let quart_height = (lighting.height / 4) as usize;
            for xz in 0..16 {
                for y in 0..quart_height {
                    let expected = if (y / 4) % 2 == 0 { "plains" } else { "forest" };
                    assert_eq!(
                        chunk.values()[xz * quart_height + y].path,
                        expected,
                        "{name}"
                    );
                }
            }
            assert!(actual.restore_saved_biomes([position, position]).is_err());
            assert!(
                actual
                    .restore_saved_biomes([ChunkPos { x: 2, z: 0 }])
                    .is_err()
            );
            assert!(
                actual
                    .restore_saved_biomes([ChunkPos { x: 100, z: 100 }])
                    .is_err()
            );
            let terrain = actual.restore_saved_terrain([position])?;
            let key = native_world::loaded::ChunkPos { x: 0, z: 0 };
            assert_eq!(
                terrain.world.block(0, lighting.min_y, 0).unwrap().name,
                "minecraft:stone"
            );
            assert_eq!(terrain.biomes[&key].values(), chunk.values());
            assert_eq!(terrain.heightmaps[&key].first_available(native_persistence::heightmap::SavedGenerationHeightmapKind::WorldSurfaceWorldgen, 0, 0), Some(lighting.min_y + 7));
            assert_eq!(
                terrain.heightmaps[&key].first_available(
                    native_persistence::heightmap::SavedGenerationHeightmapKind::WorldSurface,
                    0,
                    0
                ),
                None
            );
            assert_eq!(terrain.postprocessing[&key].as_ref().unwrap(), &marks);
            assert!(
                actual
                    .restore_saved_terrain([position, ChunkPos { x: 1, z: 0 }])
                    .is_err()
            );
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
