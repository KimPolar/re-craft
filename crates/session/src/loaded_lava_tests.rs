use super::*;
use native_resources::packs::{ResourceId, ResourceProvider};

struct FireTag(bool);
impl ResourceProvider for FireTag {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data(&mut self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(
            (path == "data/minecraft/tags/block/blocks_lava_fire_spread.json").then(|| {
                if self.0 {
                    br#"{"values":["minecraft:stone"]}"#.to_vec()
                } else {
                    br#"{"values":[]}"#.to_vec()
                }
            }),
        )
    }
}

fn lava_session(blocked: bool) -> (LoadedSession, IVec3, i32) {
    let mut session = super::tests::session();
    let ids: BTreeSet<_> = [
        "minecraft:air",
        "minecraft:void_air",
        "minecraft:stone",
        "minecraft:lava",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let tag = ResourceId::parse("minecraft:blocks_lava_fire_spread").unwrap();
    let tags = native_resources::ore_feature::OreBlockTags::load(
        &mut FireTag(blocked),
        &ids,
        &BTreeSet::from([tag]),
    )
    .unwrap();
    session.set_vanilla_block_ids(Arc::new(ids));
    session.set_lava_fire_spread_tags(Arc::new(tags));
    let seed = (0_i32..100_000)
        .map(|value| value.wrapping_mul(1_000_003))
        .find(|&value| {
            let [x, y, z] =
                native_core::random::BlockRandomPosition::new(value).next_section_offset();
            (4..=11).contains(&x) && (4..=10).contains(&y) && (4..=11).contains(&z)
        })
        .unwrap();
    let position = IVec3::from_array(
        native_core::random::BlockRandomPosition::new(seed).next_section_offset(),
    );
    let stone = session.intern_block(BlockState {
        name: "minecraft:stone".into(),
        properties: Default::default(),
    });
    let lava = session.intern_block(BlockState {
        name: "minecraft:lava".into(),
        properties: BTreeMap::from([("level".into(), "0".into())]),
    });
    for x in -3..=3 {
        for y in -1..=3 {
            for z in -3..=3 {
                let absolute = session.origin + position + IVec3::new(x, y, z);
                Arc::make_mut(&mut session.world)
                    .set_block_id(absolute.x, absolute.y, absolute.z, stone)
                    .unwrap();
            }
        }
    }
    let absolute = session.origin + position;
    Arc::make_mut(&mut session.world)
        .set_block_id(absolute.x, absolute.y, absolute.z, lava)
        .unwrap();
    session.random_block_position = native_core::random::BlockRandomPosition::new(seed);
    session.set_random_tick_speed(1);
    session.set_fire_spread_radius_around_player(-1).unwrap();
    (session, position, seed)
}

#[test]
fn lava_selection_runs_block_then_fluid_with_active_spread_tag() {
    for blocked in [false, true] {
        for seed in -20..=20 {
            let (mut session, _, _) = lava_session(blocked);
            session.level_random = native_core::random::LegacyRandom::new(seed);
            let mut expected = session.level_random.clone();
            for _ in 0..2 {
                let attempts = expected.next_int_bound(3).unwrap();
                if attempts == 0 {
                    for _ in 0..6 {
                        expected.next_int_bound(3);
                    }
                } else {
                    for _ in 0..attempts {
                        expected.next_int_bound(3);
                        expected.next_int_bound(3);
                        if blocked {
                            break;
                        }
                    }
                }
            }
            session.advance_random_block_ticks();
            assert!(
                !session.unresolved_other_random_tick,
                "seed={seed} blocked={blocked} detail={:?}",
                session.unresolved_other_random_tick_detail
            );
            assert_eq!(
                session.level_random.next_int(),
                expected.next_int(),
                "seed={seed} blocked={blocked}"
            );
        }
    }
}

#[test]
fn disabled_fire_radius_skips_lava_randomness_but_keeps_section_sampling() {
    for speed in [1, i32::MAX as u32] {
        let (mut session, _, seed) = lava_session(true);
        session.set_fire_spread_radius_around_player(0).unwrap();
        session.set_random_tick_speed(speed);
        let before = session.level_random.clone().next_int();
        let mut expected_position = native_core::random::BlockRandomPosition::new(seed);
        expected_position.skip(speed);
        session.advance_random_block_ticks();
        assert!(!session.unresolved_other_random_tick);
        assert_eq!(session.level_random.next_int(), before);
        assert_eq!(
            session.random_block_position.next_section_offset(),
            expected_position.next_section_offset()
        );
    }
}

#[test]
fn lava_ignition_that_cannot_be_preserved_blocks_save() {
    let (mut session, position, _) = lava_session(true);
    let registry = Arc::make_mut(session.vanilla_block_ids.as_mut().unwrap());
    registry.insert("minecraft:oak_planks".into());
    let seed = (0_i64..1000)
        .find(|&seed| {
            native_core::random::LegacyRandom::new(seed)
                .next_int_bound(3)
                .unwrap()
                > 0
        })
        .unwrap();
    let mut preview = native_core::random::LegacyRandom::new(seed);
    preview.next_int_bound(3);
    let dx = preview.next_int_bound(3).unwrap() - 1;
    let dz = preview.next_int_bound(3).unwrap() - 1;
    let target = session.origin + position + IVec3::new(dx, 1, dz);
    let wood = session.intern_block(BlockState {
        name: "minecraft:oak_planks".into(),
        properties: Default::default(),
    });
    Arc::make_mut(&mut session.world)
        .set_block_id(target.x, target.y, target.z, session.states[0])
        .unwrap();
    Arc::make_mut(&mut session.world)
        .set_block_id(target.x, target.y + 1, target.z, wood)
        .unwrap();
    session.level_random = native_core::random::LegacyRandom::new(seed);
    session.advance_random_block_ticks();
    assert!(session.unresolved_other_random_tick);
    assert!(
        session
            .unresolved_other_random_tick_detail
            .as_deref()
            .unwrap()
            .contains("lava ignition")
    );
    assert!(
        session
            .draft_block_ticks(session.player_chunk().unwrap())
            .is_err()
    );
}
