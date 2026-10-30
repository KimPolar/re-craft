//! Closed chest block-entity geometry in Minecraft 26.3's model coordinates.

use anyhow::{Result, bail};
use native_resources::{
    blockstates::{BlockstateDefinition, ModelChoice},
    geometry::{BakedBlockstate, CuboidModel, Direction, Quad, Vertex, WeightedModel},
    models::TextureReference,
    packs::ResourceId,
};
use native_world::loaded::BlockState;

pub fn bake(state: &BlockState) -> Result<Option<BakedBlockstate>> {
    let texture = match state.name.as_str() {
        "minecraft:chest" => "normal",
        "minecraft:trapped_chest" => "trapped",
        _ => return Ok(None),
    };
    let facing = match state.properties.get("facing").map(String::as_str) {
        Some(value @ ("north" | "south" | "west" | "east")) => value,
        _ => bail!("invalid chest facing"),
    };
    if state.properties.len() != 3
        || !matches!(
            state.properties.get("waterlogged").map(String::as_str),
            Some("true" | "false")
        )
    {
        bail!("invalid chest block state");
    }
    let kind = match state.properties.get("type").map(String::as_str) {
        Some(kind @ ("single" | "left" | "right")) => kind,
        _ => bail!("invalid chest type"),
    };
    let suffix = match kind {
        "left" => "_left",
        "right" => "_right",
        _ => "",
    };
    let sprite = ResourceId::parse(&format!("minecraft:entity/chest/{texture}{suffix}"))?;
    let choice = ModelChoice {
        model: ResourceId::parse("native:entity/chest")?,
        x_rotation: 0,
        y_rotation: 0,
        uv_lock: false,
        weight: 1,
    };
    let mut quads = Vec::with_capacity(if kind == "single" { 18 } else { 15 });
    let (x, width, excluded) = match kind {
        "left" => (0.0, 15.0, Some(Direction::West)),
        "right" => (1.0, 15.0, Some(Direction::East)),
        _ => (1.0, 14.0, None),
    };
    append_cube(
        &mut quads,
        &sprite,
        facing,
        [x, 0.0, 1.0],
        [width, 10.0, 14.0],
        [0.0, 19.0],
        excluded,
    );
    append_cube(
        &mut quads,
        &sprite,
        facing,
        [x, 9.0, 1.0],
        [width, 5.0, 14.0],
        [0.0, 0.0],
        excluded,
    );
    let lock_x = match kind {
        "left" => 0.0,
        "right" => 15.0,
        _ => 7.0,
    };
    let lock_width = if kind == "single" { 2.0 } else { 1.0 };
    append_cube(
        &mut quads,
        &sprite,
        facing,
        [lock_x, 7.0, 15.0],
        [lock_width, 4.0, 1.0],
        [0.0, 0.0],
        excluded,
    );
    Ok(Some(BakedBlockstate {
        definition: BlockstateDefinition {
            multipart: false,
            groups: vec![vec![choice.clone()]],
        },
        groups: vec![vec![WeightedModel {
            choice,
            model: CuboidModel {
                ambient_occlusion: false,
                quads,
            },
        }]],
    }))
}

fn append_cube(
    out: &mut Vec<Quad>,
    sprite: &ResourceId,
    facing: &str,
    from: [f32; 3],
    size: [f32; 3],
    uv: [f32; 2],
    excluded: Option<Direction>,
) {
    let [x0, y0, z0] = from;
    let [w, h, d] = size;
    let (x1, y1, z1) = (x0 + w, y0 + h, z0 + d);
    let [u, v] = uv;
    let (u1, u2, u3, u4, u5) = (
        u + d,
        u + d + w,
        u + d + 2.0 * w,
        u + 2.0 * d + w,
        u + 2.0 * d + 2.0 * w,
    );
    let (v1, v2) = (v + d, v + d + h);
    let faces = [
        (
            Direction::Down,
            [[x1, y0, z1], [x0, y0, z1], [x0, y0, z0], [x1, y0, z0]],
            [[u2, v], [u1, v], [u1, v1], [u2, v1]],
        ),
        (
            Direction::Up,
            [[x1, y1, z0], [x0, y1, z0], [x0, y1, z1], [x1, y1, z1]],
            [[u3, v1], [u2, v1], [u2, v], [u3, v]],
        ),
        (
            Direction::West,
            [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
            [[u1, v1], [u, v1], [u, v2], [u1, v2]],
        ),
        (
            Direction::North,
            [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
            [[u2, v1], [u1, v1], [u1, v2], [u2, v2]],
        ),
        (
            Direction::East,
            [[x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1]],
            [[u4, v1], [u2, v1], [u2, v2], [u4, v2]],
        ),
        (
            Direction::South,
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            [[u5, v1], [u4, v1], [u4, v2], [u5, v2]],
        ),
    ];
    for (direction, positions, uvs) in faces {
        if Some(direction) == excluded {
            continue;
        }
        let direction = rotate_direction(direction, facing);
        let vertices = std::array::from_fn(|i| {
            let [x, y, z] = positions[i].map(|value| value / 16.0);
            let (x, z) = match facing {
                "north" => (1.0 - x, 1.0 - z),
                "west" => (1.0 - z, x),
                "east" => (z, 1.0 - x),
                _ => (x, z),
            };
            Vertex {
                position: [x, y, z],
                uv: uvs[i].map(|value| value / 64.0),
            }
        });
        out.push(Quad {
            source_face: direction,
            direction,
            cullface: None,
            texture: TextureReference {
                sprite: sprite.clone(),
                force_translucent: false,
            },
            tint_index: -1,
            shade_direction_override: None,
            light_emission: 0,
            vertices,
        });
    }
}

fn rotate_direction(direction: Direction, facing: &str) -> Direction {
    use Direction::*;
    match (facing, direction) {
        ("north", North) => South,
        ("north", South) => North,
        ("north", West) => East,
        ("north", East) => West,
        ("west", North) => East,
        ("west", South) => West,
        ("west", West) => North,
        ("west", East) => South,
        ("east", North) => West,
        ("east", South) => East,
        ("east", West) => South,
        ("east", East) => North,
        _ => direction,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, path::PathBuf, process::Command};

    fn state(kind: &str, facing: &str) -> BlockState {
        BlockState {
            name: "minecraft:chest".into(),
            properties: BTreeMap::from([
                ("type".into(), kind.into()),
                ("facing".into(), facing.into()),
                ("waterlogged".into(), "false".into()),
            ]),
        }
    }

    #[test]
    fn chest_models_resolve_materials_and_exclude_connected_faces() {
        for (kind, count) in [("single", 18), ("left", 15), ("right", 15)] {
            for facing in ["north", "south", "west", "east"] {
                let baked = bake(&state(kind, facing)).unwrap().unwrap();
                let quads = &baked.groups[0][0].model.quads;
                assert_eq!(quads.len(), count);
                assert!(quads.iter().all(|quad| quad.cullface.is_none()));
                let expected = format!(
                    "entity/chest/normal{}",
                    if kind == "single" {
                        ""
                    } else if kind == "left" {
                        "_left"
                    } else {
                        "_right"
                    }
                );
                assert!(
                    quads
                        .iter()
                        .all(|quad| quad.texture.sprite.path == expected)
                );
            }
        }
        let mut trapped = state("single", "north");
        trapped.name = "minecraft:trapped_chest".into();
        assert_eq!(
            bake(&trapped).unwrap().unwrap().groups[0][0].model.quads[0]
                .texture
                .sprite
                .path,
            "entity/chest/trapped"
        );
        trapped.properties.insert("facing".into(), "up".into());
        assert!(bake(&trapped).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn closed_chest_geometry_and_uv_match_java() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("ChestModelOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/ChestModelOracle.java")).unwrap();
        let run = |command: &mut Command| {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "Java chest oracle failed:\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("ChestModelOracle")
            .arg(&output));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&output).unwrap()).unwrap();
        let mut index = 0;
        for kind in ["single", "left", "right"] {
            for facing in ["south", "north", "west", "east"] {
                let baked = bake(&state(kind, facing)).unwrap().unwrap();
                for quad in &baked.groups[0][0].model.quads {
                    let row = &rows[index];
                    index += 1;
                    assert_eq!(row["kind"], kind);
                    assert_eq!(row["facing"], facing);
                    for (actual, expected) in quad
                        .direction
                        .normal()
                        .into_iter()
                        .zip(row["normal"].as_array().unwrap())
                    {
                        assert!(
                            (f64::from(actual) - expected.as_f64().unwrap()).abs() < 0.000001,
                            "normal {kind} {facing} {row}"
                        );
                    }
                    for (vertex, expected) in quad
                        .vertices
                        .iter()
                        .zip(row["vertices"].as_array().unwrap())
                    {
                        for (actual, expected) in vertex
                            .position
                            .into_iter()
                            .chain(vertex.uv)
                            .zip(expected.as_array().unwrap())
                        {
                            assert!(
                                (f64::from(actual) - expected.as_f64().unwrap()).abs() < 0.000001,
                                "vertex {kind} {facing} {row}: {vertex:?}"
                            );
                        }
                    }
                }
            }
        }
        assert_eq!(index, rows.len());
        assert_eq!(index, 192);
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("ChestModelOracle")
            .arg(&output)
            .arg("animated"));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let mut index = 0;
        for openness in [0.0_f32, 0.1, 0.25, 0.5, 0.75, 1.0] {
            for kind in ["single", "left", "right"] {
                for facing in ["south", "north", "west", "east"] {
                    let baked = bake(&state(kind, facing)).unwrap().unwrap();
                    let quads = &baked.groups[0][0].model.quads;
                    let body_faces = if kind == "single" { 6 } else { 5 };
                    let base: Vec<crate::mesh::Vertex> = quads[body_faces..]
                        .iter()
                        .flat_map(|quad| quad.vertices)
                        .map(|vertex| crate::mesh::Vertex {
                            position: vertex.position,
                            uv: vertex.uv,
                            shade: 1.0,
                            tint: [1.0; 3],
                            light: 0,
                        })
                        .collect();
                    let lid = crate::preview::ChestLidMesh {
                        position: glam::IVec3::ZERO,
                        local_origin: glam::Vec3::ZERO,
                        facing: facing.into(),
                        start: 0,
                        end: base.len(),
                    };
                    let visual = native_session::chest_lid::ChestLidVisual {
                        position: glam::IVec3::ZERO,
                        previous: openness,
                        current: openness,
                    };
                    let animated = lid.animated_vertices(&base, visual.angle(0.5));
                    for face in 0..quads.len() {
                        let row = &rows[index];
                        index += 1;
                        assert_eq!(row["kind"], kind);
                        assert_eq!(row["facing"], facing);
                        if face < body_faces {
                            continue;
                        }
                        for (vertex, expected) in animated
                            [(face - body_faces) * 4..(face - body_faces + 1) * 4]
                            .iter()
                            .zip(row["vertices"].as_array().unwrap())
                        {
                            for (actual, expected) in vertex
                                .position
                                .into_iter()
                                .chain(vertex.uv)
                                .zip(expected.as_array().unwrap())
                            {
                                assert!(
                                    (f64::from(actual) - expected.as_f64().unwrap()).abs()
                                        < 0.000002,
                                    "animated {openness} {kind} {facing} {row}: {:?}",
                                    vertex.position
                                );
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(index, 1152);
        assert_eq!(index, rows.len());
    }
}
