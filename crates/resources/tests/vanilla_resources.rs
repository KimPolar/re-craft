use std::collections::BTreeMap;

use native_resources::{
    blockstates, default_minecraft_dir, geometry, models,
    packs::{ResourceId, ResourceStack},
    sprites::SpriteAtlas,
};

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation"]
fn vanilla_blockstates_and_models_resolve() {
    let dir = std::env::var_os("MINECRAFT_DIR")
        .map(Into::into)
        .unwrap_or_else(|| default_minecraft_dir().unwrap());
    let mut resources = ResourceStack::open(&dir).unwrap();
    let grass = models::resolve_model(&mut resources, "block/grass_block").unwrap();
    assert_eq!(grass.fields["elements"].as_array().unwrap().len(), 2);
    let baked_grass = geometry::bake_untransformed(&grass).unwrap();
    assert_eq!(baked_grass.quads.len(), 10);
    assert_eq!(
        baked_grass
            .quads
            .iter()
            .filter(|quad| quad.tint_index == 0)
            .count(),
        5
    );
    assert_eq!(
        grass.texture("top").unwrap().sprite.path,
        "block/grass_block_top"
    );
    let grass_properties = BTreeMap::from([("snowy".to_owned(), "false".to_owned())]);
    let grass_choices =
        blockstates::resolve_blockstate(&mut resources, "grass_block", &grass_properties).unwrap();
    assert_eq!(grass_choices.len(), 1);
    assert_eq!(grass_choices[0].len(), 4);
    let grass_baked = geometry::bake_blockstate(&mut resources, "grass_block", &grass_properties)
        .unwrap()
        .groups;
    assert_eq!(grass_baked.len(), 1);
    assert_eq!(grass_baked[0].len(), 4);
    assert!(
        grass_baked[0]
            .iter()
            .all(|variant| variant.model.quads.len() == 10)
    );

    let stairs = BTreeMap::from([
        ("facing".to_owned(), "east".to_owned()),
        ("half".to_owned(), "bottom".to_owned()),
        ("shape".to_owned(), "straight".to_owned()),
        ("waterlogged".to_owned(), "false".to_owned()),
    ]);
    let choices = blockstates::resolve_blockstate(&mut resources, "oak_stairs", &stairs).unwrap();
    assert_eq!(choices[0][0].model.path, "block/oak_stairs");
    let stairs_baked = geometry::bake_blockstate(&mut resources, "oak_stairs", &stairs)
        .unwrap()
        .groups;
    assert_eq!(stairs_baked.len(), 1);
    assert!(stairs_baked[0][0].model.quads.len() > 6);
    let stairs_model = models::resolve_model(&mut resources, "block/oak_stairs").unwrap();
    let baked_stairs = geometry::bake_untransformed(&stairs_model).unwrap();
    assert!(baked_stairs.quads.len() > 6);
    assert!(
        baked_stairs
            .quads
            .iter()
            .any(|quad| quad.cullface.is_none())
    );
    let water = ResourceId::parse("block/water_still").unwrap();
    let lava = ResourceId::parse("block/lava_still").unwrap();
    let textures = baked_grass
        .quads
        .iter()
        .chain(&baked_stairs.quads)
        .map(|quad| quad.texture.sprite.clone())
        .chain([water.clone(), lava.clone()]);
    let mut atlas = SpriteAtlas::build(&mut resources, textures, 1024).unwrap();
    assert_eq!(atlas.rectangle(&water).unwrap().width, 16);
    assert!(atlas.frames(&water).unwrap().len() > 1);
    assert!(atlas.frames(&lava).unwrap().len() > 1);
    assert!(atlas.update(2));

    let fence = BTreeMap::from([
        ("north".to_owned(), "true".to_owned()),
        ("south".to_owned(), "false".to_owned()),
        ("east".to_owned(), "false".to_owned()),
        ("west".to_owned(), "false".to_owned()),
        ("waterlogged".to_owned(), "false".to_owned()),
    ]);
    assert_eq!(
        blockstates::resolve_blockstate(&mut resources, "oak_fence", &fence)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        geometry::bake_blockstate(&mut resources, "oak_fence", &fence)
            .unwrap()
            .groups
            .len(),
        2
    );
    let flower = geometry::bake_blockstate(&mut resources, "poppy", &BTreeMap::new()).unwrap();
    assert_eq!(flower.groups[0][0].model.quads.len(), 4);
    assert!(
        flower.groups[0][0].model.quads[0]
            .vertices
            .iter()
            .any(|vertex| vertex.position[0] > 0.01 && vertex.position[0] < 0.99)
    );
    let torch = geometry::bake_blockstate(
        &mut resources,
        "wall_torch",
        &BTreeMap::from([("facing".to_owned(), "north".to_owned())]),
    )
    .unwrap();
    assert_eq!(torch.groups[0][0].model.quads.len(), 6);
}
