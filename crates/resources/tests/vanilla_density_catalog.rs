//! Inspect which installed registry definitions can be compiled today.
use native_resources::{
    density::{DensityCompiler, WorldgenRandom},
    packs::{ResourceProvider, ResourceStack},
};
use std::path::PathBuf;

#[test]
#[ignore = "requires Minecraft Java Edition 26.3"]
fn catalog_compiles_installed_density_functions() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let mut sources = ResourceStack::open(&game).unwrap();
    let ids = sources.list_data("worldgen/density_function").unwrap();
    let mut compiler = DensityCompiler::with_world_seed(&mut sources, 0, WorldgenRandom::Xoroshiro);
    let mut supported = Vec::new();
    for id in ids {
        let name = format!("{}:{}", id.namespace, id.path);
        if compiler.compile_id(&name).is_ok() {
            supported.push(name);
        }
    }
    println!("supported: {} {:?}", supported.len(), supported);
    for expected in [
        "minecraft:overworld/ore_vein/toggle",
        "minecraft:overworld/ore_vein/richness",
        "minecraft:overworld/caves/spaghetti_2d_thickness_modulator",
        "minecraft:shift_x",
        "minecraft:shift_z",
        "minecraft:overworld/temperature",
        "minecraft:overworld/vegetation",
        "minecraft:overworld/continents",
        "minecraft:overworld/erosion",
        "minecraft:overworld_large_biomes/temperature",
        "minecraft:overworld_large_biomes/vegetation",
        "minecraft:overworld_large_biomes/continents",
        "minecraft:overworld_large_biomes/erosion",
    ] {
        assert!(
            supported.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }
}
