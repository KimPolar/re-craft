use anyhow::{Context, Result, ensure};
use native_resources::speleothem::{self, SpeleothemWorld, Thickness, VerticalDirection};
use serde_json::Value;
use std::collections::BTreeMap;
use std::{path::PathBuf, process::Command};

#[derive(Clone, Copy)]
enum State {
    Base,
    Stone,
    Other,
    Air,
}
struct Draft {
    support_y: i32,
    support: State,
    events: Vec<String>,
}

#[derive(Clone, Copy)]
enum ClusterState {
    Air,
    Stone,
    Lava,
    Water,
    Base,
    Pointed(bool),
}
struct ClusterDraft {
    profile: i32,
    blocks: BTreeMap<[i32; 3], ClusterState>,
    events: Vec<String>,
}
impl SpeleothemWorld for ClusterDraft {
    type State = ClusterState;
    fn block(&mut self, pos: [i32; 3]) -> Result<ClusterState> {
        Ok(*self
            .blocks
            .get(&pos)
            .unwrap_or(&if self.profile == 1 && pos[1] == 10 {
                ClusterState::Stone
            } else if pos[1] <= 5 {
                ClusterState::Stone
            } else if self.profile != 2 && pos[1] >= 14 {
                if self.profile == 3 {
                    ClusterState::Lava
                } else {
                    ClusterState::Stone
                }
            } else {
                ClusterState::Air
            }))
    }
    fn is_base(&mut self, state: &ClusterState) -> Result<bool> {
        Ok(matches!(state, ClusterState::Stone | ClusterState::Base))
    }
    fn is_replaceable(&mut self, state: &ClusterState) -> Result<bool> {
        Ok(matches!(state, ClusterState::Stone))
    }
    fn is_water_at(&mut self, pos: [i32; 3]) -> Result<bool> {
        Ok(matches!(
            self.block(pos)?,
            ClusterState::Water | ClusterState::Pointed(true)
        ))
    }
    fn set_base(&mut self, pos: [i32; 3]) -> Result<()> {
        self.blocks.insert(pos, ClusterState::Base);
        self.events
            .push(format!("{}:{}:{}:base:2", pos[0], pos[1], pos[2]));
        Ok(())
    }
    fn set_pointed(
        &mut self,
        pos: [i32; 3],
        direction: VerticalDirection,
        thickness: Thickness,
        waterlogged: bool,
    ) -> Result<()> {
        let direction = if direction == VerticalDirection::Up {
            "up"
        } else {
            "down"
        };
        let thickness = match thickness {
            Thickness::Base => "base",
            Thickness::Middle => "middle",
            Thickness::Frustum => "frustum",
            Thickness::Tip => "tip",
            Thickness::TipMerge => "tip_merge",
        };
        self.blocks.insert(pos, ClusterState::Pointed(waterlogged));
        self.events.push(format!(
            "{}:{}:{}:{direction}:{thickness}:{waterlogged}:2",
            pos[0], pos[1], pos[2]
        ));
        Ok(())
    }
}
impl speleothem::ClusterWorld for ClusterDraft {
    fn is_empty_or_water(&mut self, state: &ClusterState) -> Result<bool> {
        Ok(matches!(state, ClusterState::Air | ClusterState::Water))
    }
    fn is_lava(&mut self, state: &ClusterState) -> Result<bool> {
        Ok(matches!(state, ClusterState::Lava))
    }
    fn can_place_pool(&mut self, pos: [i32; 3]) -> Result<bool> {
        if matches!(
            self.block(pos)?,
            ClusterState::Water | ClusterState::Base | ClusterState::Pointed(_)
        ) || self.is_water_at([pos[0], pos[1] + 1, pos[2]])?
        {
            return Ok(false);
        }
        for offset in [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0], [0, -1, 0]] {
            let neighbor = [pos[0] + offset[0], pos[1] + offset[1], pos[2] + offset[2]];
            if !matches!(
                self.block(neighbor)?,
                ClusterState::Stone | ClusterState::Water | ClusterState::Pointed(true)
            ) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn set_water(&mut self, pos: [i32; 3]) -> Result<()> {
        self.blocks.insert(pos, ClusterState::Water);
        self.events
            .push(format!("{}:{}:{}:water:2", pos[0], pos[1], pos[2]));
        Ok(())
    }
}
struct Numbers(f32, bool);
impl speleothem::ClusterNumbers for Numbers {
    fn height(&self, random: &mut native_core::random::DecorationRandom) -> i32 {
        if self.1 {
            1 + random.next_int_bound(4).unwrap()
        } else {
            4
        }
    }
    fn wetness(&self, _: &mut native_core::random::DecorationRandom) -> f32 {
        self.0
    }
    fn density(&self, random: &mut native_core::random::DecorationRandom) -> f32 {
        if self.1 {
            0.3 + random.next_float() * (0.7 - 0.3)
        } else {
            0.7
        }
    }
    fn radius(&self, random: &mut native_core::random::DecorationRandom) -> i32 {
        if self.1 {
            1 + random.next_int_bound(3).unwrap()
        } else {
            2
        }
    }
    fn layer(&self, random: &mut native_core::random::DecorationRandom) -> i32 {
        if self.1 {
            1 + random.next_int_bound(3).unwrap()
        } else {
            2
        }
    }
}
impl SpeleothemWorld for Draft {
    type State = State;
    fn block(&mut self, pos: [i32; 3]) -> Result<State> {
        Ok(if pos[1] == self.support_y {
            self.support
        } else {
            State::Air
        })
    }
    fn is_base(&mut self, state: &State) -> Result<bool> {
        Ok(matches!(state, State::Base | State::Stone))
    }
    fn is_replaceable(&mut self, state: &State) -> Result<bool> {
        Ok(matches!(state, State::Stone))
    }
    fn is_water_at(&mut self, pos: [i32; 3]) -> Result<bool> {
        Ok(pos[1].rem_euclid(2) == 0)
    }
    fn set_base(&mut self, _: [i32; 3]) -> Result<()> {
        self.events.push("2".into());
        Ok(())
    }
    fn set_pointed(
        &mut self,
        pos: [i32; 3],
        direction: VerticalDirection,
        thickness: Thickness,
        waterlogged: bool,
    ) -> Result<()> {
        let direction = match direction {
            VerticalDirection::Up => "up",
            VerticalDirection::Down => "down",
        };
        let thickness = match thickness {
            Thickness::Base => "base",
            Thickness::Middle => "middle",
            Thickness::Frustum => "frustum",
            Thickness::Tip => "tip",
            Thickness::TipMerge => "tip_merge",
        };
        self.events.push(format!(
            "{}:{direction}:{thickness}:{waterlogged}:2",
            pos[1]
        ));
        Ok(())
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn speleothem_columns_match_java() -> Result<()> {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
    let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
    let temp = tempfile::tempdir()?;
    let version = game.join("versions/26.3");
    let manifest: Value = serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
    ensure!(manifest["id"] == "26.3", "wrong Minecraft version");
    let mut paths = vec![version.join("26.3.jar"), temp.path().to_owned()];
    for library in manifest["libraries"]
        .as_array()
        .context("libraries missing")?
    {
        if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
            let path = game.join("libraries").join(path);
            if path.is_file() {
                paths.push(path);
            }
        }
    }
    let classpath = std::env::join_paths(paths)?;
    let source = temp.path().join("SpeleothemOracle.java");
    std::fs::write(&source, include_str!("java/SpeleothemOracle.java"))?;
    let output = Command::new(java.join("javac.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg(&source)
        .output()?;
    ensure!(
        output.status.success(),
        "Java compilation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(java.join("java.exe"))
        .arg("-cp")
        .arg(&classpath)
        .arg("SpeleothemOracle")
        .output()?;
    ensure!(
        output.status.success(),
        "Java oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut columns = 0;
    let mut bases = 0;
    let mut scans = 0;
    let mut chances = 0;
    let mut heights = 0;
    let mut clusters = 0;
    for line in String::from_utf8(output.stdout)?.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["C", direction, height, merge, support, events] => {
                let direction = if *direction == "0" {
                    VerticalDirection::Up
                } else {
                    VerticalDirection::Down
                };
                let support_y = if direction == VerticalDirection::Up {
                    9
                } else {
                    11
                };
                let support = match *support {
                    "0" => State::Base,
                    "1" => State::Stone,
                    "2" => State::Other,
                    "3" => State::Air,
                    _ => anyhow::bail!("bad support"),
                };
                let mut draft = Draft {
                    support_y,
                    support,
                    events: vec![],
                };
                speleothem::grow(
                    &mut draft,
                    [4, 10, 7],
                    direction,
                    height.parse()?,
                    *merge == "1",
                )?;
                assert_eq!(
                    if draft.events.is_empty() {
                        "-".into()
                    } else {
                        draft.events.join(",")
                    },
                    *events,
                    "{line}"
                );
                columns += 1;
            }
            ["B", supported, expected, events] => {
                let mut draft = Draft {
                    support_y: 10,
                    support: if *supported == "1" {
                        State::Stone
                    } else {
                        State::Other
                    },
                    events: vec![],
                };
                assert_eq!(
                    speleothem::place_base_if_possible(&mut draft, [4, 10, 7])?,
                    expected.parse::<bool>()?
                );
                assert_eq!(
                    if draft.events.is_empty() {
                        "-".into()
                    } else {
                        draft.events.join(",")
                    },
                    *events
                );
                bases += 1;
            }
            ["S", profile, range, floor, ceiling, visits] => {
                let profile: i32 = profile.parse()?;
                let mut visited = Vec::new();
                let column = speleothem::scan_column([4, 10, 7], range.parse()?, &mut |pos| {
                    let y = pos[1];
                    visited.push(y);
                    Ok(match profile {
                        0 => y > 7 && y < 14,
                        1 => true,
                        2 => y != 10,
                        3 => y > 8,
                        4 => y < 12,
                        _ => unreachable!(),
                    })
                })?;
                let render = |v: Option<i32>| v.map_or_else(|| "_".to_owned(), |y| y.to_string());
                let (actual_floor, actual_ceiling) = column.map_or_else(
                    || ("absent".to_owned(), "absent".to_owned()),
                    |c| (render(c.floor), render(c.ceiling)),
                );
                assert_eq!(
                    (&*actual_floor, &*actual_ceiling),
                    (*floor, *ceiling),
                    "{line}"
                );
                assert_eq!(format!("{visited:?}").replace(", ", ","), *visits, "{line}");
                scans += 1;
            }
            ["P", rx, rz, x, z, bits] => {
                let actual = speleothem::edge_chance(
                    rx.parse()?,
                    rz.parse()?,
                    x.parse()?,
                    z.parse()?,
                    3,
                    0.1,
                );
                assert_eq!(actual.to_bits() as i64, bits.parse::<i64>()?, "{line}");
                chances += 1;
            }
            ["H", x, z, height, density, seed, result, next] => {
                let density = match *density {
                    "0" => 0.0,
                    "1" => 0.3,
                    "2" => 1.0,
                    _ => anyhow::bail!("bad density"),
                };
                let mut random = native_core::random::DecorationRandom::new(seed.parse()?);
                assert_eq!(
                    speleothem::column_height(
                        &mut random,
                        x.parse()?,
                        z.parse()?,
                        density,
                        height.parse()?,
                        8,
                        3
                    ),
                    result.parse::<i32>()?,
                    "{line}"
                );
                assert_eq!(random.next_long(), next.parse::<i64>()?, "{line}");
                heights += 1;
            }
            ["G", variable, wet, profile, seed, result, next, events] => {
                let mut draft = ClusterDraft {
                    profile: profile.parse()?,
                    blocks: BTreeMap::new(),
                    events: vec![],
                };
                let config = speleothem::ClusterConfig {
                    search_range: 12,
                    height_difference: 1,
                    height_deviation: 3,
                    edge_chance: 0.1,
                    edge_distance: 3,
                    center_distance: 8,
                };
                let mut random = native_core::random::DecorationRandom::new(seed.parse()?);
                assert_eq!(
                    config.place(
                        &mut draft,
                        &Numbers(wet.parse()?, *variable == "1"),
                        &mut random,
                        [0, 10, 0]
                    )?,
                    result.parse::<bool>()?,
                    "{line}"
                );
                assert_eq!(random.next_long(), next.parse::<i64>()?, "{line}");
                assert_eq!(
                    if draft.events.is_empty() {
                        "-".into()
                    } else {
                        draft.events.join(",")
                    },
                    *events,
                    "{line}"
                );
                clusters += 1;
            }
            _ => {}
        }
    }
    assert_eq!(columns, 2 * 8 * 2 * 4);
    assert_eq!(bases, 2);
    assert_eq!(scans, 5 * 7);
    assert_eq!(chances, 3 * 3 * 5 * 5);
    assert_eq!(heights, 5 * 5 * 3 * 3 * 16);
    assert_eq!(clusters, 2 * 2 * 4 * 16);
    Ok(())
}
