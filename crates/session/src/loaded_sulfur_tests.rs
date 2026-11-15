use super::*;
use native_resources::packs::{ResourceId, ResourceProvider};

struct SpikeTag(bool);
impl ResourceProvider for SpikeTag {
    fn read_asset(&mut self, _: &str) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn read_data_stack(&mut self, _: &str) -> anyhow::Result<Vec<Vec<u8>>> {
        Ok(vec![if self.0 {
            br#"{"values":["minecraft:sulfur_spike"]}"#.to_vec()
        } else {
            br#"{"values":[]}"#.to_vec()
        }])
    }
}

#[test]
fn sampled_sulfur_spike_uses_level_rng_and_blocks_unimplemented_growth() {
    let position_seed = (0_i32..100_000)
        .map(|value| value.wrapping_mul(1_000_003))
        .find(|&value| {
            let [x, y, z] =
                native_core::random::BlockRandomPosition::new(value).next_section_offset();
            (4..=11).contains(&x) && (4..=10).contains(&y) && (4..=11).contains(&z)
        })
        .unwrap();
    let relative = IVec3::from_array(
        native_core::random::BlockRandomPosition::new(position_seed).next_section_offset(),
    );
    let low_seed = (0_i64..100_000)
        .map(|value| value * 1_000_003)
        .find(|&value| native_core::random::LegacyRandom::new(value).next_float() < 0.011_377_778)
        .unwrap();
    let ids: BTreeSet<_> = [
        "minecraft:air",
        "minecraft:stone",
        "minecraft:sulfur",
        "minecraft:sulfur_spike",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let tag = ResourceId::parse("minecraft:speleothems").unwrap();
    for (tagged, sulfur, unresolved) in [
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let mut session = super::tests::session();
        session.set_vanilla_block_ids(Arc::new(ids.clone()));
        session.set_speleothem_tags(Arc::new(
            native_resources::ore_feature::OreBlockTags::load(
                &mut SpikeTag(tagged),
                &ids,
                &BTreeSet::from([tag.clone()]),
            )
            .unwrap(),
        ));
        let spike = session.intern_block(BlockState {
            name: "minecraft:sulfur_spike".into(),
            properties: BTreeMap::from([
                ("vertical_direction".into(), "down".into()),
                ("thickness".into(), "tip".into()),
                ("waterlogged".into(), "false".into()),
            ]),
        });
        let support = session.intern_block(BlockState {
            name: if sulfur {
                "minecraft:sulfur"
            } else {
                "minecraft:stone"
            }
            .into(),
            properties: Default::default(),
        });
        let absolute = session.origin + relative;
        Arc::make_mut(&mut session.world)
            .set_block_id(absolute.x, absolute.y, absolute.z, spike)
            .unwrap();
        Arc::make_mut(&mut session.world)
            .set_block_id(absolute.x, absolute.y + 1, absolute.z, support)
            .unwrap();
        session.random_block_position =
            native_core::random::BlockRandomPosition::new(position_seed);
        session.level_random = native_core::random::LegacyRandom::new(low_seed);
        let mut expected = session.level_random.clone();
        expected.next_float();
        session.set_random_tick_speed(1);
        session.advance_random_block_ticks();
        assert_eq!(
            session.unresolved_other_random_tick, unresolved,
            "tagged={tagged} sulfur={sulfur}"
        );
        assert_eq!(session.level_random.next_int(), expected.next_int());
        if unresolved {
            assert!(
                session
                    .unresolved_other_random_tick_detail
                    .as_deref()
                    .unwrap()
                    .contains("sulfur spike growth")
            );
            assert!(
                session
                    .draft_block_ticks(session.player_chunk().unwrap())
                    .is_err()
            );
        }
    }
}
