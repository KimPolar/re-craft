//! Apply speleothem clusters to a transactional generation region.
use super::*;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    speleothem::{
        ClusterWorld, SingleWorld, SpeleothemBlocks, SpeleothemWorld, Thickness, VerticalDirection,
    },
};

pub(super) fn place_cluster(
    region: &mut DecorationRegion<'_>,
    feature: &SpeleothemClusterFeature<BlockState>,
    random: &mut DecorationRandom,
    origin: [i32; 3],
) -> Result<bool> {
    let result = (|| {
        // The write effects of these vanilla states are checked against 26.3.
        // Other configured blocks must not be published until their effects
        // and pointed-state properties are known as well.
        ensure!(
            feature.base_id() == &ResourceId::parse("minecraft:sulfur")?
                && feature.pointed_id() == &ResourceId::parse("minecraft:sulfur_spike")?,
            "unsupported speleothem block effects"
        );
        let registry = region
            .template_registry
            .context("speleothem block registry missing")?;
        let mut world = SpeleothemRegion {
            region,
            feature: &feature.blocks,
            registry,
            water_tag: ResourceId::parse("minecraft:water")?,
            stone_tag: ResourceId::parse("minecraft:base_stone_overworld")?,
        };
        feature.config.place(&mut world, feature, random, origin)
    })();
    if result.is_err() {
        region.failed = true;
    }
    result
}

pub(super) fn place_single(
    region: &mut DecorationRegion<'_>,
    feature: &SpeleothemFeature<BlockState>,
    random: &mut DecorationRandom,
    origin: [i32; 3],
) -> Result<bool> {
    let result = (|| {
        ensure!(
            feature.blocks.base_id() == &ResourceId::parse("minecraft:sulfur")?
                && feature.blocks.pointed_id() == &ResourceId::parse("minecraft:sulfur_spike")?,
            "unsupported speleothem block effects"
        );
        let registry = region
            .template_registry
            .context("speleothem block registry missing")?;
        let mut world = SpeleothemRegion {
            region,
            feature: &feature.blocks,
            registry,
            water_tag: ResourceId::parse("minecraft:water")?,
            stone_tag: ResourceId::parse("minecraft:base_stone_overworld")?,
        };
        feature.place(&mut world, random, origin)
    })();
    if result.is_err() {
        region.failed = true;
    }
    result
}

struct SpeleothemRegion<'a, 'b> {
    region: &'a mut DecorationRegion<'b>,
    feature: &'a SpeleothemBlocks<BlockState>,
    registry: &'a GeneratedBlockRegistry,
    water_tag: ResourceId,
    stone_tag: ResourceId,
}

impl SpeleothemRegion<'_, '_> {
    fn fluid_is_water(&mut self, state: &BlockState) -> Result<bool> {
        DiskWorld::fluid_in_tag(self.region, state, &self.water_tag)
    }

    fn set_block(&mut self, pos: [i32; 3], state: &BlockState) -> Result<()> {
        if !self.region.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.region.world.chunks.contains_key(&chunk(pos)),
            "speleothem write chunk is missing"
        );
        if self.region.is_outside(pos[1]) {
            return Ok(());
        }
        let old = self.region.read(pos)?;
        ensure!(
            self.registry.is_canonical_state(&old.name, &old.properties)
                && self.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some()
                    || generated_ore_state(&old)?
                    || matches!(
                        old.name.as_str(),
                        "minecraft:sulfur" | "minecraft:sulfur_spike"
                    )),
            "unsupported speleothem source effects at {pos:?}: {old:?}"
        );
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties)
                && self.registry.has_block_entity(&state.name) == Some(false),
            "invalid speleothem replacement at {pos:?}: {state:?}"
        );
        self.region.set_proto_block(pos, state)
    }

    fn default_state(&self, name: &str) -> Result<BlockState> {
        Ok(BlockState {
            name: name.to_owned(),
            properties: self
                .registry
                .default_properties(name)
                .with_context(|| format!("missing default state {name}"))?,
        })
    }
}

impl SpeleothemWorld for SpeleothemRegion<'_, '_> {
    type State = BlockState;

    fn block(&mut self, pos: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.region, pos)
    }

    fn is_base(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name == "minecraft:sulfur" || self.is_replaceable(state)?)
    }

    fn is_replaceable(&mut self, state: &BlockState) -> Result<bool> {
        if let Some(tag) = self.feature.replaceable_tag() {
            return self.region.tags.contains(tag, &state.name);
        }
        let id = ResourceId::parse(&state.name)?;
        Ok(self
            .feature
            .replaceable_blocks()
            .context("missing speleothem replaceable blocks")?
            .contains(&id))
    }

    fn is_water_at(&mut self, pos: [i32; 3]) -> Result<bool> {
        let state = self.block(pos)?;
        self.fluid_is_water(&state)
    }

    fn set_base(&mut self, pos: [i32; 3]) -> Result<()> {
        let state = self.default_state("minecraft:sulfur")?;
        self.set_block(pos, &state)
    }

    fn set_pointed(
        &mut self,
        pos: [i32; 3],
        direction: VerticalDirection,
        thickness: Thickness,
        waterlogged: bool,
    ) -> Result<()> {
        let mut state = self.default_state("minecraft:sulfur_spike")?;
        state.properties.insert(
            "vertical_direction".into(),
            match direction {
                VerticalDirection::Up => "up",
                VerticalDirection::Down => "down",
            }
            .into(),
        );
        state.properties.insert(
            "thickness".into(),
            match thickness {
                Thickness::Base => "base",
                Thickness::Middle => "middle",
                Thickness::Frustum => "frustum",
                Thickness::Tip => "tip",
                Thickness::TipMerge => "tip_merge",
            }
            .into(),
        );
        state
            .properties
            .insert("waterlogged".into(), waterlogged.to_string());
        self.set_block(pos, &state)
    }
}

impl ClusterWorld for SpeleothemRegion<'_, '_> {
    fn is_empty_or_water(&mut self, state: &BlockState) -> Result<bool> {
        Ok(is_air(state) || state.name == "minecraft:water")
    }

    fn is_lava(&mut self, state: &BlockState) -> Result<bool> {
        Ok(state.name == "minecraft:lava")
    }

    fn can_place_pool(&mut self, pos: [i32; 3]) -> Result<bool> {
        let state = self.block(pos)?;
        if matches!(
            state.name.as_str(),
            "minecraft:water" | "minecraft:sulfur" | "minecraft:sulfur_spike"
        ) {
            return Ok(false);
        }
        let above = self.block([pos[0], pos[1].wrapping_add(1), pos[2]])?;
        if self.fluid_is_water(&above)? {
            return Ok(false);
        }
        for [dx, dy, dz] in [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0], [0, -1, 0]] {
            let next = self.block([
                pos[0].wrapping_add(dx),
                pos[1].wrapping_add(dy),
                pos[2].wrapping_add(dz),
            ])?;
            if !self.region.tags.contains(&self.stone_tag, &next.name)?
                && !self.fluid_is_water(&next)?
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn set_water(&mut self, pos: [i32; 3]) -> Result<()> {
        let water = self.default_state("minecraft:water")?;
        self.set_block(pos, &water)
    }
}

impl SingleWorld for SpeleothemRegion<'_, '_> {
    fn is_empty_or_water(&mut self, state: &BlockState) -> Result<bool> {
        Ok(is_air(state) || state.name == "minecraft:water")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn solid_predicate_covers_sulfur_states_and_rejects_malformed_states() -> Result<()> {
        let (mut terrain, tags, motion, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        for name in ["minecraft:sulfur", "minecraft:cinnabar"] {
            assert!(DiskWorld::is_solid(
                &mut region,
                &BlockState {
                    name: name.into(),
                    properties: BTreeMap::new()
                }
            )?);
            assert!(!DiskWorld::is_liquid(
                &mut region,
                &BlockState {
                    name: name.into(),
                    properties: BTreeMap::new()
                }
            )?);
        }
        for direction in ["up", "down"] {
            for thickness in ["tip_merge", "tip", "frustum", "middle", "base"] {
                for waterlogged in ["true", "false"] {
                    let state = BlockState {
                        name: "minecraft:sulfur_spike".into(),
                        properties: BTreeMap::from([
                            ("vertical_direction".into(), direction.into()),
                            ("thickness".into(), thickness.into()),
                            ("waterlogged".into(), waterlogged.into()),
                        ]),
                    };
                    assert!(DiskWorld::is_solid(&mut region, &state)?);
                    assert!(!DiskWorld::is_liquid(&mut region, &state)?);
                }
            }
        }
        let malformed = BlockState {
            name: "minecraft:sulfur_spike".into(),
            properties: BTreeMap::new(),
        };
        assert!(DiskWorld::is_solid(&mut region, &malformed).is_err());
        assert!(DiskWorld::is_liquid(&mut region, &malformed).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn cluster_region_applies_water_and_pointed_states_and_rejects_unknown_effects() -> Result<()> {
        let game = native_resources::default_minecraft_dir()?;
        let mut resources = native_resources::packs::ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let placed = PlacedDecorationFeature::load(
            &mut resources,
            &ResourceId::parse("minecraft:sulfur_spike")?,
            &mut |v| {
                let (name, properties) = registry.resolve(v)?;
                Ok(BlockState { name, properties })
            },
        )?;
        assert!(placed.needs_water_fluids());
        assert!(placed.required_tags().contains(&ResourceId::parse(
            "minecraft:sulfur_spike_replaceable_blocks"
        )?));
        let feature = SpeleothemClusterFeature::decode(
            &json!({
                "type":"minecraft:speleothem_cluster", "base_block":"minecraft:sulfur",
                "pointed_block":"minecraft:sulfur_spike", "replaceable_blocks":["minecraft:stone"],
                "floor_to_ceiling_search_range":12, "height":4, "radius":1,
                "max_stalagmite_stalactite_height_diff":1, "height_deviation":3,
                "speleothem_block_layer_thickness":2, "density":1.0, "wetness":1.0,
                "chance_of_speleothem_at_max_distance_from_center":1.0,
                "max_distance_from_edge_affecting_chance_of_speleothem":3,
                "max_distance_from_center_affecting_height_bias":8
            }),
            &mut |v| {
                let (name, properties) = registry.resolve(v)?;
                Ok(BlockState { name, properties })
            },
        )?;
        let names = [
            "air",
            "cave_air",
            "void_air",
            "stone",
            "diamond_ore",
            "water",
            "sulfur",
            "sulfur_spike",
            "chest",
        ]
        .into_iter()
        .map(|v| format!("minecraft:{v}"))
        .collect();
        let motion = HeightmapTags::load(&mut super::super::tests::Data, &names)?;
        let tags = OreBlockTags::load(
            &mut super::super::tests::Data,
            &names,
            &feature.required_tags(),
        )?;
        let fluids = BTreeSet::from([
            "minecraft:water".to_owned(),
            "minecraft:flowing_water".to_owned(),
        ]);
        let (mut terrain, _, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        region.template_registry = Some(&registry);
        region.lake_water_fluids = Some(&fluids);
        let single = SpeleothemFeature::decode(
            &json!({"type":"minecraft:speleothem", "base_block":"minecraft:sulfur", "pointed_block":"minecraft:sulfur_spike", "replaceable_blocks":["minecraft:stone"]}),
            &mut |v| {
                let (name, properties) = registry.resolve(v)?;
                Ok(BlockState { name, properties })
            },
        )?;
        assert!(place_single(
            &mut region,
            &single,
            &mut DecorationRandom::new(0),
            [12, 8, 12]
        )?);
        assert_eq!(
            OreWorld::block(&mut region, [12, 8, 12])?.name,
            "minecraft:sulfur_spike"
        );
        {
            let mut adapter = SpeleothemRegion {
                region: &mut region,
                feature: &feature.blocks,
                registry: &registry,
                water_tag: ResourceId::parse("minecraft:water")?,
                stone_tag: ResourceId::parse("minecraft:base_stone_overworld")?,
            };
            assert!(adapter.can_place_pool([8, 7, 8])?);
            adapter.set_water([8, 7, 8])?;
            assert!(!adapter.can_place_pool([8, 7, 8])?);
            adapter.set_pointed([8, 8, 7], VerticalDirection::Up, Thickness::Tip, true)?;
            assert!(adapter.is_water_at([8, 8, 7])?);
            assert!(!adapter.can_place_pool([8, 7, 7])?);
        }
        let pointed = OreWorld::block(&mut region, [8, 8, 7])?;
        assert_eq!(pointed.name, "minecraft:sulfur_spike");
        assert!(DiskWorld::is_solid(&mut region, &pointed)?);
        let sulfur = registry.resolve(&json!("minecraft:sulfur"))?;
        assert!(DiskWorld::is_solid(
            &mut region,
            &BlockState {
                name: sulfur.0,
                properties: sulfur.1
            }
        )?);
        assert!(DiskWorld::is_solid(
            &mut region,
            &BlockState {
                name: "minecraft:cinnabar".into(),
                properties: BTreeMap::new()
            }
        )?);
        assert_eq!(
            pointed
                .properties
                .get("vertical_direction")
                .map(String::as_str),
            Some("up")
        );
        assert_eq!(
            pointed.properties.get("waterlogged").map(String::as_str),
            Some("true")
        );
        region.finish()?;
        assert_eq!(terrain.snapshot().block(8, 8, 7).unwrap(), &pointed);

        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        region.template_registry = Some(&registry);
        region.lake_water_fluids = Some(&fluids);
        let chest = registry.resolve(&json!("minecraft:chest"))?;
        region.set_proto_block(
            [3, 7, 3],
            &BlockState {
                name: chest.0,
                properties: chest.1,
            },
        )?;
        let result = place_cluster(
            &mut region,
            &feature,
            &mut DecorationRandom::new(0),
            [4, 8, 4],
        );
        let error = result.expect_err("chest replacement must abort the generation draft");
        assert!(
            format!("{error:#}").contains("unsupported speleothem source effects"),
            "unexpected generation error: {error:#}"
        );
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }
}
