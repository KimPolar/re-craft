//! Conservative block-light recomputation for redstone ore state changes.
//!
//! Only complete neighborhoods of known, full-cube/air states are accepted.
//! Unknown states or light data leave the existing light arrays untouched.

use crate::loaded::{BlockPosition, BlockState, ChunkPos, World};
use std::{
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    sync::Arc,
};

const DIRECTIONS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

fn properties(state: &BlockState) -> Option<(u8, u8)> {
    let pair = match state.name.as_str() {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
            if state.properties.is_empty() =>
        {
            (0, 0)
        }
        "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore"
            if state.properties.len() == 1 =>
        {
            let emission = match state.properties.get("lit")?.as_str() {
                "true" => 9,
                "false" => 0,
                _ => return None,
            };
            (15, emission)
        }
        "minecraft:stone"
        | "minecraft:deepslate"
        | "minecraft:dirt"
        | "minecraft:granite"
        | "minecraft:diorite"
        | "minecraft:andesite"
        | "minecraft:tuff"
        | "minecraft:coal_ore"
        | "minecraft:deepslate_coal_ore"
        | "minecraft:iron_ore"
        | "minecraft:deepslate_iron_ore"
        | "minecraft:copper_ore"
        | "minecraft:deepslate_copper_ore"
        | "minecraft:gold_ore"
        | "minecraft:deepslate_gold_ore"
        | "minecraft:diamond_ore"
        | "minecraft:deepslate_diamond_ore"
        | "minecraft:lapis_ore"
        | "minecraft:deepslate_lapis_ore"
        | "minecraft:emerald_ore"
        | "minecraft:deepslate_emerald_ore"
            if state.properties.is_empty() =>
        {
            (15, 0)
        }
        _ => return None,
    };
    Some(pair)
}

fn offset(p: BlockPosition, d: (i32, i32, i32)) -> Option<BlockPosition> {
    Some(BlockPosition {
        x: p.x.checked_add(d.0)?,
        y: p.y.checked_add(d.1)?,
        z: p.z.checked_add(d.2)?,
    })
}

/// Recompute light after a redstone ore changes. A change in opacity can
/// affect light from a level-15 source up to fourteen blocks away.
/// Returns None if any necessary state or light array is unavailable.
/// On success the returned chunks are exactly those whose light changed.
pub fn relight_redstone(
    world: &mut World,
    center: BlockPosition,
    opacity_changed: bool,
) -> Option<BTreeSet<ChunkPos>> {
    let radius: i32 = if opacity_changed { 14 } else { 8 };
    let mut cells = BTreeMap::new();
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs() + dy.abs() + dz.abs() > radius {
                    continue;
                }
                let p = offset(center, (dx, dy, dz))?;
                if world.chunks.get(&p.chunk())?.status.as_deref() != Some("minecraft:full") {
                    return None;
                }
                let (opacity, emission) = properties(world.block(p.x, p.y, p.z)?)?;
                let previous = world.light(p.x, p.y, p.z, false)?;
                cells.insert(p, (opacity, emission, previous, emission));
            }
        }
    }
    let mut queue = BinaryHeap::new();
    for (&p, cell) in &mut cells {
        for direction in DIRECTIONS {
            let next = offset(p, direction)?;
            if (next.x - center.x).abs() + (next.y - center.y).abs() + (next.z - center.z).abs()
                <= radius
            {
                continue;
            }
            if world.chunks.get(&next.chunk())?.status.as_deref() != Some("minecraft:full") {
                return None;
            }
            properties(world.block(next.x, next.y, next.z)?)?;
            let boundary = world.light(next.x, next.y, next.z, false)?;
            cell.3 = cell.3.max(boundary.saturating_sub(cell.0.max(1)));
        }
        if cell.3 > 0 {
            queue.push((cell.3, p));
        }
    }
    while let Some((level, p)) = queue.pop() {
        if cells.get(&p)?.3 != level {
            continue;
        }
        for direction in DIRECTIONS {
            let next = offset(p, direction)?;
            let Some(cell) = cells.get_mut(&next) else {
                continue;
            };
            let value = level.saturating_sub(cell.0.max(1));
            if value > cell.3 {
                cell.3 = value;
                queue.push((value, next));
            }
        }
    }
    // All reads and validation have completed before the first mutation.
    let mut changed = BTreeSet::new();
    for (p, (_, _, previous, value)) in cells {
        if previous == value {
            continue;
        }
        let chunk_pos = p.chunk();
        let chunk = Arc::make_mut(world.chunks.get_mut(&chunk_pos)?);
        let section_y = i8::try_from(p.y.div_euclid(16)).ok()?;
        let section = Arc::make_mut(chunk.sections.get_mut(&section_y)?);
        let data = section.block_light.as_mut()?;
        let index = p.x.rem_euclid(16) as usize
            + p.z.rem_euclid(16) as usize * 16
            + p.y.rem_euclid(16) as usize * 256;
        let byte = &mut data[index / 2];
        if index % 2 == 0 {
            *byte = (*byte & 0xf0) | value;
        } else {
            *byte = (*byte & 0x0f) | (value << 4);
        }
        changed.insert(chunk_pos);
    }
    Some(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loaded::{BLOCK_COUNT, BlockState, Chunk, LIGHT_BYTES, Section};

    fn position(x: i32, y: i32, z: i32) -> BlockPosition {
        BlockPosition { x, y, z }
    }
    fn state(name: &str, lit: Option<bool>) -> BlockState {
        BlockState {
            name: format!("minecraft:{name}"),
            properties: lit
                .map(|v| [("lit".to_owned(), v.to_string())].into())
                .unwrap_or_default(),
        }
    }
    fn fixture() -> (
        World,
        crate::loaded::BlockStateId,
        crate::loaded::BlockStateId,
        crate::loaded::BlockStateId,
    ) {
        let mut world = World::new("minecraft:overworld");
        let air = world.intern_state(state("air", None));
        let unlit = world.intern_state(state("redstone_ore", Some(false)));
        let lit = world.intern_state(state("redstone_ore", Some(true)));
        let stone = world.intern_state(state("stone", None));
        for x in 0..=1 {
            for z in 0..=1 {
                let chunk_pos = ChunkPos { x, z };
                let mut chunk = Chunk {
                    position: chunk_pos,
                    status: Some("minecraft:full".into()),
                    sections: BTreeMap::new(),
                };
                for y in 0..=1 {
                    chunk.sections.insert(
                        y,
                        Arc::new(Section {
                            blocks: Some(Box::new([air; BLOCK_COUNT])),
                            biomes: None,
                            block_light: Some(Box::new([0; LIGHT_BYTES])),
                            sky_light: None,
                        }),
                    );
                }
                world.chunks.insert(chunk_pos, Arc::new(chunk));
            }
        }
        (world, unlit, lit, stone)
    }
    fn set(world: &mut World, p: BlockPosition, id: crate::loaded::BlockStateId) {
        world.set_block_id(p.x, p.y, p.z, id).unwrap();
    }
    fn light(world: &World, p: BlockPosition) -> u8 {
        world.light(p.x, p.y, p.z, false).unwrap()
    }

    #[test]
    fn activating_and_extinguishing_preserves_other_sources_and_snapshots() {
        let (mut world, unlit, lit, stone) = fixture();
        let center = position(15, 15, 15);
        let other = position(20, 15, 15);
        let wall = position(15, 16, 15);
        set(&mut world, center, unlit);
        set(&mut world, other, lit);
        set(&mut world, wall, stone);
        relight_redstone(&mut world, other, false).unwrap();
        let old = world.clone();
        set(&mut world, center, lit);
        let changed = relight_redstone(&mut world, center, false).unwrap();
        assert!(changed.len() > 1, "light crosses chunk boundaries");
        assert_eq!(light(&world, center), 9);
        assert_eq!(light(&world, position(14, 15, 15)), 8);
        assert_eq!(light(&world, wall), 0);
        assert_eq!(
            light(&world, position(15, 17, 15)),
            5,
            "light goes around the wall"
        );
        assert_eq!(light(&old, center), 0);
        set(&mut world, center, unlit);
        relight_redstone(&mut world, center, false).unwrap();
        assert_eq!(
            light(&world, position(16, 15, 15)),
            5,
            "the other source remains"
        );
        assert_eq!(light(&world, position(14, 15, 15)), 1);
    }

    #[test]
    fn unknown_data_aborts_without_partial_light_changes() {
        let (mut world, _, lit, _) = fixture();
        let center = position(15, 15, 15);
        set(&mut world, center, lit);
        let unknown = world.intern_state(state("glass", None));
        set(&mut world, position(14, 15, 15), unknown);
        let before = world.clone();
        assert!(relight_redstone(&mut world, center, false).is_none());
        for (key, chunk) in &world.chunks {
            assert!(Arc::ptr_eq(chunk, &before.chunks[key]));
        }
        assert_eq!(light(&world, center), 0);
    }

    #[test]
    fn removing_a_lit_source_clears_its_light() {
        let (mut world, _, lit, _) = fixture();
        let center = position(15, 15, 15);
        set(&mut world, center, lit);
        relight_redstone(&mut world, center, false).unwrap();
        let air = world.intern_state(state("air", None));
        set(&mut world, center, air);
        relight_redstone(&mut world, center, true).unwrap();
        assert_eq!(light(&world, center), 0);
        assert_eq!(light(&world, position(16, 15, 15)), 0);
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn supported_properties_match_vanilla() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BlockLightPropertiesOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/BlockLightPropertiesOracle.java"),
        )
        .unwrap();
        let run = |cmd: &mut Command| {
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        let mut states = Vec::new();
        for name in [
            "air",
            "cave_air",
            "void_air",
            "stone",
            "deepslate",
            "dirt",
            "granite",
            "diorite",
            "andesite",
            "tuff",
            "coal_ore",
            "deepslate_coal_ore",
            "iron_ore",
            "deepslate_iron_ore",
            "copper_ore",
            "deepslate_copper_ore",
            "gold_ore",
            "deepslate_gold_ore",
            "diamond_ore",
            "deepslate_diamond_ore",
            "lapis_ore",
            "deepslate_lapis_ore",
            "emerald_ore",
            "deepslate_emerald_ore",
        ] {
            states.push(state(name, None));
        }
        for name in ["redstone_ore", "deepslate_redstone_ore"] {
            for lit in [false, true] {
                states.push(state(name, Some(lit)));
            }
        }
        let cases: Vec<_> = states
            .iter()
            .map(|s| {
                let mut row = serde_json::json!({"name": s.name});
                if let Some(lit) = s.properties.get("lit") {
                    row["lit"] = serde_json::json!(lit == "true");
                }
                row
            })
            .collect();
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("BlockLightPropertiesOracle")
            .arg(&input)
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), states.len());
        for (s, row) in states.iter().zip(rows) {
            let (opacity, emission) = properties(s).unwrap();
            assert_eq!(row, serde_json::json!([opacity, emission, false]), "{s:?}");
        }
    }
}
