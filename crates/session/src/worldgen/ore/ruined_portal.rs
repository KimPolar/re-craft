//! Apply ruined-portal terrain decoration to an unpublished generation region.
use super::*;
use native_resources::{
    block_registry::GeneratedBlockRegistry,
    ruined_portal_decoration::{PortalBlock, PortalDecorationWorld},
    structure_start::Heightmap,
};

struct PortalRegion<'r, 'a, 'g> {
    region: &'r mut DecorationRegion<'a>,
    registry: &'g GeneratedBlockRegistry,
    protected: &'g BTreeSet<String>,
}

impl PortalDecorationWorld for PortalRegion<'_, '_, '_> {
    type State = BlockState;

    fn block(&mut self, position: [i32; 3]) -> Result<BlockState> {
        OreWorld::block(self.region, position)
    }
    fn id<'a>(&self, state: &'a BlockState) -> &'a str {
        &state.name
    }
    fn is_air(&self, state: &BlockState) -> bool {
        super::is_air(state)
    }
    fn is_protected(&self, state: &BlockState) -> Result<bool> {
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified portal block state: {state:?}"
        );
        Ok(self.protected.contains(&state.name))
    }
    fn height(&mut self, x: i32, z: i32, map: Heightmap) -> Result<i32> {
        match map {
            Heightmap::WorldSurface => self.region.world_surface_height(x, z),
            Heightmap::OceanFloor => OreVeinWorld::ocean_floor_height(self.region, x, z),
        }
    }
    fn collision_face_full(&mut self, pos: [i32; 3], direction: [i32; 3]) -> Result<bool> {
        let state = self.block(pos)?;
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties),
            "unverified portal collision state"
        );
        let shape = native_world::collision::block_shape(&state)
            .with_context(|| format!("unsupported portal collision shape for {}", state.name))?;
        Ok(native_world::collision::covers_full_face(
            &shape,
            glam::IVec3::from_array(direction),
        ))
    }
    fn set_block(&mut self, pos: [i32; 3], block: PortalBlock) -> Result<()> {
        if !self.region.writable.contains(&chunk(pos)) {
            return Ok(());
        }
        ensure!(
            self.region.world.chunks.contains_key(&chunk(pos)),
            "portal block chunk missing"
        );
        if self.region.is_outside(pos[1]) {
            return Ok(());
        }
        let old = self.region.read(pos)?;
        let portal_state = matches!(
            old.name.as_str(),
            "minecraft:gold_block"
                | "minecraft:obsidian"
                | "minecraft:crying_obsidian"
                | "minecraft:stone_bricks"
                | "minecraft:cracked_stone_bricks"
                | "minecraft:mossy_stone_bricks"
                | "minecraft:stone_brick_stairs"
                | "minecraft:mossy_stone_brick_stairs"
                | "minecraft:stone_slab"
                | "minecraft:stone_brick_slab"
                | "minecraft:mossy_stone_brick_slab"
                | "minecraft:stone_brick_wall"
                | "minecraft:mossy_stone_brick_wall"
                | "minecraft:iron_bars"
                | "minecraft:vine"
                | "minecraft:jungle_leaves"
        );
        ensure!(
            self.registry.is_canonical_state(&old.name, &old.properties)
                && self.registry.has_block_entity(&old.name) == Some(false)
                && (disk_safe_solid(&old.name).is_some() || portal_state)
                && !self.region.monster_entities.contains_key(&pos)
                && !self.region.bee_nests.contains_key(&pos),
            "unsupported portal replacement source at {pos:?}: {old:?}"
        );
        let (name, properties) = match block {
            PortalBlock::Netherrack => ("minecraft:netherrack", BTreeMap::new()),
            PortalBlock::Magma => ("minecraft:magma_block", BTreeMap::new()),
            PortalBlock::PersistentJungleLeaves => (
                "minecraft:jungle_leaves",
                BTreeMap::from([
                    ("distance".into(), "7".into()),
                    ("persistent".into(), "true".into()),
                    ("waterlogged".into(), "false".into()),
                ]),
            ),
            PortalBlock::Vine(direction) => {
                let face = match direction {
                    [0, 0, -1] => "north",
                    [1, 0, 0] => "east",
                    [0, 0, 1] => "south",
                    [-1, 0, 0] => "west",
                    _ => bail!("invalid portal vine face"),
                };
                let mut props: BTreeMap<String, String> = ["up", "north", "south", "east", "west"]
                    .into_iter()
                    .map(|name| (name.into(), "false".into()))
                    .collect();
                props.insert(face.into(), "true".into());
                ("minecraft:vine", props)
            }
        };
        let state = BlockState {
            name: name.into(),
            properties,
        };
        ensure!(
            self.registry
                .is_canonical_state(&state.name, &state.properties)
                && self.registry.has_block_entity(&state.name) == Some(false),
            "unverified portal output state"
        );
        self.region.set_proto_block(pos, &state)
    }
}

impl DecorationRegion<'_> {
    /// Run the piece's post-template decoration on this discardable region.
    /// `protected` must be resolved from the same active block registry and packs.
    pub fn decorate_ruined_portal(
        &mut self,
        piece: &super::super::ruined_portal::SavedRuinedPortalPiece,
        registry: &GeneratedBlockRegistry,
        protected: &BTreeSet<String>,
        random: &mut DecorationRandom,
    ) -> Result<()> {
        ensure!(!self.failed, "generation attempt previously failed");
        self.failed = true;
        piece.decorate(
            &mut PortalRegion {
                region: self,
                registry,
                protected,
            },
            random,
        )?;
        self.failed = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_resources::{
        heightmap_tags::{HeightmapTags, resolve_block_tag},
        packs::ResourceStack,
        ruined_portal::{GenerationPoint, InitialSelection, Setup, VerticalPlacement},
        structure_transform::{Mirror, Rotation, bounding_box},
    };

    fn piece() -> Result<super::super::super::ruined_portal::SavedRuinedPortalPiece> {
        let size = [5, 4, 5];
        let pos = [6, 7, 6];
        let pivot = [2, 0, 2];
        let selection = InitialSelection {
            setup_index: 0,
            air_pocket: false,
            template: ResourceId::parse("minecraft:ruined_portal/portal_1")?,
            rotation: Rotation::None,
            mirror: Mirror::None,
        };
        let setup = Setup {
            placement: VerticalPlacement::OnLandSurface,
            air_pocket_probability: 0.0,
            mossiness: 0.0,
            overgrown: false,
            vines: false,
            can_be_cold: false,
            replace_with_blackstone: false,
            weight: 1.0,
        };
        let bounds = bounding_box(size, pos, selection.mirror, selection.rotation, pivot)?;
        let point = GenerationPoint {
            position: pos,
            pivot,
            bounds,
        };
        super::super::super::ruined_portal::SavedRuinedPortalPiece::capture(
            &selection, &setup, &point, size, false,
        )
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3"]
    fn portal_decoration_updates_draft_and_rolls_back_unsupported_entity() -> Result<()> {
        use native_world::loaded::BlockStateId;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game)?;
        let registry = resources.vanilla_generated_block_registry()?;
        let ids = resources.vanilla_block_ids()?;
        let protected = resolve_block_tag(
            &mut resources,
            &ResourceId::parse("minecraft:features_cannot_replace")?,
            &ids,
        )?;
        let motion = HeightmapTags::load(&mut resources, &ids)?;
        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let writable = terrain.snapshot().chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        region.decorate_ruined_portal(&piece()?, &registry, &protected, &mut random)?;
        assert_eq!(region.world_surface_height(8, 8)?, 8);
        region.finish()?;
        assert!((-6..=22).any(|x| (-6..=22).any(|z| {
            terrain.snapshot().block(x, 7, z).is_some_and(|state| {
                matches!(
                    state.name.as_str(),
                    "minecraft:netherrack" | "minecraft:magma_block"
                )
            })
        })));

        let (mut terrain, tags, _, _) = super::super::tests::fixture();
        let world = Arc::make_mut(&mut terrain.world);
        let chest = BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("facing".into(), "north".into()),
                ("type".into(), "single".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        };
        let id: BlockStateId = world.intern_state(chest);
        let chunk = Arc::make_mut(world.chunks.get_mut(&ChunkPos { x: 0, z: 0 }).unwrap());
        let section = Arc::make_mut(chunk.sections.get_mut(&0).unwrap());
        section.blocks.as_mut().unwrap()[8 + 8 * 16 + 7 * 256] = id;
        let before = terrain.snapshot();
        let writable = before.chunks.keys().copied().collect();
        let mut region = terrain.decoration_region(&tags, &motion, writable)?;
        let mut random = DecorationRandom::new(42);
        let mut expected = random.clone();
        // A data pack may remove chests from the protected tag. The world
        // adapter must then reject an unsupported block-entity replacement.
        let mut unprotected = protected.clone();
        assert!(unprotected.remove("minecraft:chest"));
        assert!(
            region
                .decorate_ruined_portal(&piece()?, &registry, &unprotected, &mut random)
                .is_err()
        );
        assert_eq!(random.next_long(), expected.next_long());
        assert!(region.finish().is_err());
        assert!(Arc::ptr_eq(&before, &terrain.snapshot()));
        Ok(())
    }
}
