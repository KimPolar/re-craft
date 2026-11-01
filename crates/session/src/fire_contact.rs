//! Vanilla full-block inside-contact detection for fire and lit campfires.
use std::collections::HashSet;

use glam::{DVec3, IVec3};
use native_world::{loaded::BlockState, movement::BoundingBox};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FireBlock {
    Fire(u8),
    Campfire(u8),
}

pub(crate) fn effect(state: &BlockState) -> Option<FireBlock> {
    match state.name.as_str() {
        "minecraft:fire" => Some(FireBlock::Fire(1)),
        "minecraft:soul_fire" => Some(FireBlock::Fire(2)),
        "minecraft:campfire" if state.properties.get("lit").is_some_and(|v| v == "true") => {
            Some(FireBlock::Campfire(1))
        }
        "minecraft:soul_campfire" if state.properties.get("lit").is_some_and(|v| v == "true") => {
            Some(FireBlock::Campfire(2))
        }
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Contact {
    pub axis_step: u8,
    pub time: f64,
    pub kind: FireBlock,
    pub position: IVec3,
}

/// The block traversal groups contacts by the leading face's grid crossings,
/// not by the exact point at which a fluid surface was intersected.
pub(crate) fn traversal_step(
    bounds: BoundingBox,
    actual: DVec3,
    requested: DVec3,
    axis_step: u8,
    position: IVec3,
) -> i64 {
    let axes = if requested.x.abs() < requested.z.abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let index = usize::from(axis_step);
    let axis = axes[index];
    if actual[axis].abs() < f64::from(1.0e-5_f32) {
        return 0;
    }
    let mut offset = DVec3::ZERO;
    for earlier in &axes[..index] {
        offset[*earlier] = actual[*earlier];
    }
    let inset = f64::from(1.0e-5_f32);
    let positive = actual[axis] > 0.0;
    let leading = if positive {
        bounds.max[axis] + offset[axis] - inset
    } else {
        bounds.min[axis] + offset[axis] + inset
    };
    let first = leading.floor() as i64;
    let cell = i64::from(position[axis]);
    (if positive { cell - first } else { first - cell }).max(0)
}

/// The callback returns None for unavailable cells and Some(None) for a
/// loaded cell without one of these interactions.
pub(crate) fn swept(
    bounds: BoundingBox,
    actual: DVec3,
    requested: DVec3,
    lookup: impl Fn(IVec3) -> Option<Option<FireBlock>>,
) -> Option<Vec<Contact>> {
    if !bounds.min.is_finite()
        || !bounds.max.is_finite()
        || !actual.is_finite()
        || !requested.is_finite()
        || !bounds.min.cmple(bounds.max).all()
    {
        return None;
    }
    let axes = if requested.x.abs() < requested.z.abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let inset = DVec3::splat(f64::from(1.0e-5_f32));
    let mut position = BoundingBox {
        min: bounds.min + inset,
        max: bounds.max - inset,
    };
    if !position.min.cmple(position.max).all() {
        return None;
    }
    let mut visited = HashSet::new();
    let mut effects = Vec::new();
    let mut work = 0_u64;
    for (axis_step, axis) in axes.into_iter().enumerate() {
        let mut delta = DVec3::ZERO;
        delta[axis] = actual[axis];
        if delta == DVec3::ZERO && actual != DVec3::ZERO {
            continue;
        }
        let search = position.expanded_towards(delta);
        if !search.min.is_finite()
            || !search.max.is_finite()
            || !search.min.cmpge(DVec3::splat(f64::from(i32::MIN))).all()
            || !search.max.cmple(DVec3::splat(f64::from(i32::MAX))).all()
        {
            return None;
        }
        let min = search.min.floor().as_ivec3();
        let max = search.max.floor().as_ivec3();
        let volume = ((i64::from(max.x) - i64::from(min.x) + 1) as u64)
            .checked_mul((i64::from(max.y) - i64::from(min.y) + 1) as u64)?
            .checked_mul((i64::from(max.z) - i64::from(min.z) + 1) as u64)?;
        work = work.checked_add(volume)?;
        if work > 4096 {
            return None;
        }
        let mut hits = Vec::new();
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let block = IVec3::new(x, y, z);
                    if visited.contains(&block) {
                        continue;
                    }
                    let base = block.as_dvec3();
                    let target = BoundingBox {
                        min: base,
                        max: base + DVec3::ONE,
                    };
                    let Some(time) = position.swept_contact_time(delta, target) else {
                        continue;
                    };
                    if let Some(kind) = lookup(block)? {
                        hits.push(Contact {
                            axis_step: axis_step as u8,
                            time,
                            kind,
                            position: block,
                        });
                    }
                }
            }
        }
        hits.sort_by(|a, b| {
            a.time
                .total_cmp(&b.time)
                .then_with(|| a.position.to_array().cmp(&b.position.to_array()))
        });
        for hit in hits {
            if visited.insert(hit.position) {
                effects.push(hit);
            }
        }
        position = position.moved(delta);
        if actual == DVec3::ZERO {
            break;
        }
    }
    Some(effects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swept_contacts_find_crossings_and_reject_missing_cells() {
        let bounds = BoundingBox {
            min: DVec3::new(0.2, 1.0, 0.2),
            max: DVec3::new(0.8, 2.8, 0.8),
        };
        let lookup = |pos| Some((pos == IVec3::new(2, 1, 0)).then_some(FireBlock::Fire(1)));
        let events = swept(
            bounds,
            DVec3::new(4.0, 0.0, 0.0),
            DVec3::new(4.0, 0.0, 0.0),
            lookup,
        )
        .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].position, IVec3::new(2, 1, 0));
        assert_eq!(events[0].kind, FireBlock::Fire(1));
        assert!((events[0].time - 0.3).abs() < 0.000_01);
        assert!(
            swept(
                bounds,
                DVec3::new(4.0, 0.0, 0.0),
                DVec3::new(4.0, 0.0, 0.0),
                |pos| if pos == IVec3::new(2, 1, 0) {
                    None
                } else {
                    Some(None)
                }
            )
            .is_none()
        );
    }

    #[test]
    fn fire_and_campfire_states_select_the_correct_damage() {
        use std::collections::BTreeMap;
        let state = |name: &str, lit: &str| BlockState {
            name: name.into(),
            properties: BTreeMap::from([("lit".into(), lit.into())]),
        };
        assert_eq!(
            effect(&state("minecraft:fire", "false")),
            Some(FireBlock::Fire(1))
        );
        assert_eq!(
            effect(&state("minecraft:soul_fire", "false")),
            Some(FireBlock::Fire(2))
        );
        assert_eq!(
            effect(&state("minecraft:campfire", "true")),
            Some(FireBlock::Campfire(1))
        );
        assert_eq!(
            effect(&state("minecraft:soul_campfire", "true")),
            Some(FireBlock::Campfire(2))
        );
        assert_eq!(effect(&state("minecraft:campfire", "false")), None);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn swept_positions_and_steps_match_java() {
        use std::{collections::BTreeMap, path::PathBuf, process::Command};
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
        let classpath = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("PlayerFireTraversalOracle.java");
        let result = temp.path().join("traversal.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/PlayerFireTraversalOracle.java"),
        )
        .unwrap();
        let compile = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "{}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&classpath)
            .arg("PlayerFireTraversalOracle")
            .arg(&result)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(rows.len(), 8);
        for row in rows {
            let input: Vec<f64> = row["input"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let position = DVec3::new(input[0], input[1], input[2]);
            let delta = DVec3::new(input[3], input[4], input[5]);
            let bounds = crate::movement::player_bounds(position, crate::PLAYER_HEIGHT);
            let contacts = swept(bounds, delta, delta, |_| Some(Some(FireBlock::Fire(1)))).unwrap();
            let mut actual = BTreeMap::new();
            for contact in contacts {
                let step =
                    traversal_step(bounds, delta, delta, contact.axis_step, contact.position);
                actual.insert(contact.position.to_array(), step);
            }
            let expected: BTreeMap<[i32; 3], i64> = row["visits"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    let v = v.as_array().unwrap();
                    (
                        [
                            v[0].as_i64().unwrap() as i32,
                            v[1].as_i64().unwrap() as i32,
                            v[2].as_i64().unwrap() as i32,
                        ],
                        v[3].as_i64().unwrap(),
                    )
                })
                .collect();
            assert_eq!(actual, expected, "{row}");
        }
    }
}
