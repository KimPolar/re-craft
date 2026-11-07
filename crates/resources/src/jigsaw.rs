//! Jigsaw connector orientation, attachment and selection order for Java 26.3.
use std::cmp::Reverse;

use anyhow::{Context, Result, bail, ensure};

use crate::{
    packs::ResourceId,
    structure_transform::{Mirror, Rotation, block_world_position},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Down,
    Up,
    North,
    South,
    West,
    East,
}

impl Direction {
    fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "down" => Self::Down,
            "up" => Self::Up,
            "north" => Self::North,
            "south" => Self::South,
            "west" => Self::West,
            "east" => Self::East,
            _ => bail!("invalid jigsaw direction {value}"),
        })
    }

    pub fn opposite(self) -> Self {
        match self {
            Self::Down => Self::Up,
            Self::Up => Self::Down,
            Self::North => Self::South,
            Self::South => Self::North,
            Self::West => Self::East,
            Self::East => Self::West,
        }
    }

    pub fn step(self) -> [i32; 3] {
        match self {
            Self::Down => [0, -1, 0],
            Self::Up => [0, 1, 0],
            Self::North => [0, 0, -1],
            Self::South => [0, 0, 1],
            Self::West => [-1, 0, 0],
            Self::East => [1, 0, 0],
        }
    }

    fn transform(self, mirror: Mirror, rotation: Rotation) -> Self {
        let [x, y, z] = self.step();
        let [x, _, z] =
            crate::structure_transform::transform_block([x, y, z], mirror, rotation, [0; 3]);
        match [x, y, z] {
            [0, -1, 0] => Self::Down,
            [0, 1, 0] => Self::Up,
            [0, 0, -1] => Self::North,
            [0, 0, 1] => Self::South,
            [-1, 0, 0] => Self::West,
            [1, 0, 0] => Self::East,
            _ => unreachable!("rotation preserves cardinal directions"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Up => "up",
            Self::North => "north",
            Self::South => "south",
            Self::West => "west",
            Self::East => "east",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Orientation {
    pub front: Direction,
    pub top: Direction,
}

impl Orientation {
    pub fn parse(value: &str) -> Result<Self> {
        let (front, top) = value
            .split_once('_')
            .context("invalid jigsaw orientation")?;
        let front = Direction::parse(front)?;
        let top = Direction::parse(top)?;
        ensure!(
            match front {
                Direction::Up | Direction::Down => matches!(
                    top,
                    Direction::North | Direction::South | Direction::East | Direction::West
                ),
                _ => top == Direction::Up,
            },
            "invalid jigsaw orientation {value}"
        );
        Ok(Self { front, top })
    }

    pub fn rotate(self, rotation: Rotation) -> Self {
        self.transform(Mirror::None, rotation)
    }

    pub fn transform(self, mirror: Mirror, rotation: Rotation) -> Self {
        Self {
            front: self.front.transform(mirror, rotation),
            top: self.top.transform(mirror, rotation),
        }
    }

    pub fn name(self) -> String {
        format!("{}_{}", self.front.name(), self.top.name())
    }

    pub fn default_joint(self) -> Joint {
        match self.front {
            Direction::Up | Direction::Down => Joint::Rollable,
            _ => Joint::Aligned,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Joint {
    Aligned,
    Rollable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Connector {
    pub position: [i32; 3],
    pub orientation: Orientation,
    pub joint: Joint,
    /// `None` represents Java's explicit null-name wildcard.
    pub name: Option<ResourceId>,
    pub target: ResourceId,
    pub pool: ResourceId,
    pub placement_priority: i32,
    pub selection_priority: i32,
}

impl Connector {
    pub fn transformed(&self, origin: [i32; 3], rotation: Rotation) -> Self {
        let mut result = self.clone();
        result.position =
            block_world_position(self.position, origin, Mirror::None, rotation, [0; 3]);
        result.orientation = self.orientation.rotate(rotation);
        result
    }

    /// Java consults the parent joint only; a null child name is a wildcard.
    pub fn can_attach(&self, child: &Self) -> bool {
        self.orientation.front == child.orientation.front.opposite()
            && (self.joint == Joint::Rollable || self.orientation.top == child.orientation.top)
            && child.name.as_ref().is_none_or(|name| name == &self.target)
    }

    pub fn adjacent_position(&self) -> [i32; 3] {
        let step = self.orientation.front.step();
        std::array::from_fn(|index| self.position[index].wrapping_add(step[index]))
    }
}

/// Shuffle before stable descending selection-priority sort, as Java does.
pub fn shuffle_by_selection_priority(
    connectors: &mut [Connector],
    random: &mut impl FnMut(i32) -> Option<i32>,
) -> Result<()> {
    let count = i32::try_from(connectors.len()).context("too many jigsaw connectors")?;
    for end in (2..=count).rev() {
        let index = random(end).context("invalid jigsaw random bound")?;
        ensure!(
            (0..end).contains(&index),
            "jigsaw random result out of bounds"
        );
        connectors.swap((end - 1) as usize, index as usize);
    }
    connectors.sort_by_key(|item| Reverse(item.selection_priority));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_core::random::LegacyRandom;

    const ORIENTATIONS: [&str; 12] = [
        "down_east",
        "down_north",
        "down_south",
        "down_west",
        "up_east",
        "up_north",
        "up_south",
        "up_west",
        "west_up",
        "east_up",
        "north_up",
        "south_up",
    ];
    const ROTATIONS: [Rotation; 4] = [
        Rotation::None,
        Rotation::Clockwise90,
        Rotation::Clockwise180,
        Rotation::Counterclockwise90,
    ];

    fn connector(
        orientation: Orientation,
        name: Option<&str>,
        priority: i32,
        index: i32,
    ) -> Connector {
        Connector {
            position: [index, 0, 0],
            orientation,
            joint: Joint::Aligned,
            name: name.map(|name| ResourceId::parse(name).unwrap()),
            target: ResourceId::parse("example:door").unwrap(),
            pool: ResourceId::parse("minecraft:empty").unwrap(),
            placement_priority: 0,
            selection_priority: priority,
        }
    }

    #[test]
    fn connector_transform_attachment_and_priority() -> Result<()> {
        assert!(Orientation::parse("north_down").is_err());
        assert!(Orientation::parse("up_up").is_err());
        let mut parent = connector(Orientation::parse("up_north")?, Some("example:door"), 0, 2);
        parent.position = [2, 4, -3];
        let child = connector(Orientation::parse("down_west")?, Some("example:door"), 0, 0);
        assert!(!parent.can_attach(&child));
        parent.joint = Joint::Rollable;
        assert!(parent.can_attach(&child));
        assert_eq!(parent.adjacent_position(), [2, 5, -3]);
        let rotated = parent.transformed([10, 0, -10], Rotation::Clockwise90);
        assert_eq!(rotated.position, [13, 4, -8]);
        assert_eq!(rotated.orientation, Orientation::parse("up_east")?);
        let mut list = vec![
            connector(parent.orientation, None, 1, 0),
            connector(parent.orientation, None, 2, 1),
            connector(parent.orientation, None, 2, 2),
        ];
        let mut bounds = Vec::new();
        shuffle_by_selection_priority(&mut list, &mut |bound| {
            bounds.push(bound);
            Some(0)
        })?;
        assert_eq!(bounds, [3, 2]);
        assert_eq!(
            list.iter().map(|item| item.position[0]).collect::<Vec<_>>(),
            [1, 2, 0]
        );
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn connector_rules_and_priority_match_java() -> Result<()> {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::default_minecraft_dir().unwrap());
        let _verified = crate::packs::ResourceStack::open(&game)?;
        let temp = tempfile::tempdir()?;
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json"))?)?;
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
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
        let cp = std::env::join_paths(paths)?;
        let java = std::env::var_os("MINECRAFT_JAVA_HOME")
            .or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from)
            .context("Java 25 home is missing")?
            .join("bin");
        let source = temp.path().join("JigsawConnectorOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawConnectorOracle.java"),
        )?;
        let compiled = Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&source)
            .output()?;
        ensure!(
            compiled.status.success(),
            "Java compile failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("JigsawConnectorOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut counts = [0; 4];
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["D", a, joint] => {
                    let orientation = Orientation::parse(ORIENTATIONS[a.parse::<usize>()?])?;
                    assert_eq!(
                        orientation.default_joint(),
                        if *joint == "aligned" {
                            Joint::Aligned
                        } else {
                            Joint::Rollable
                        }
                    );
                    counts[0] += 1;
                }
                ["R", a, rotation, expected] => {
                    let orientation = Orientation::parse(ORIENTATIONS[a.parse::<usize>()?])?;
                    assert_eq!(
                        orientation.rotate(ROTATIONS[rotation.parse::<usize>()?]),
                        Orientation::parse(expected)?
                    );
                    counts[1] += 1;
                }
                ["A", a, b, joint, name, expected] => {
                    let mut parent = connector(
                        Orientation::parse(ORIENTATIONS[a.parse::<usize>()?])?,
                        Some("example:door"),
                        0,
                        0,
                    );
                    parent.joint = if *joint == "0" {
                        Joint::Aligned
                    } else {
                        Joint::Rollable
                    };
                    let child_name = match *name {
                        "0" => Some("example:door"),
                        "1" => Some("example:other"),
                        _ => None,
                    };
                    let child = connector(
                        Orientation::parse(ORIENTATIONS[b.parse::<usize>()?])?,
                        child_name,
                        0,
                        0,
                    );
                    assert_eq!(parent.can_attach(&child), expected.parse::<bool>()?);
                    counts[2] += 1;
                }
                ["S", seed, expected, next] => {
                    let priorities = [2, -1, 0, 2, 0, i32::MAX, i32::MIN, 2];
                    let orientation = Orientation::parse(ORIENTATIONS[0])?;
                    let mut list: Vec<_> = priorities
                        .into_iter()
                        .enumerate()
                        .map(|(i, priority)| connector(orientation, None, priority, i as i32))
                        .collect();
                    let mut random = LegacyRandom::new(seed.parse()?);
                    shuffle_by_selection_priority(&mut list, &mut |bound| {
                        random.next_int_bound(bound)
                    })?;
                    let actual = list
                        .iter()
                        .map(|item| char::from_digit(item.position[0] as u32, 10).unwrap())
                        .collect::<String>();
                    assert_eq!(actual.as_str(), *expected);
                    assert_eq!(random.next_long(), next.parse::<i64>()?);
                    counts[3] += 1;
                }
                _ => {}
            }
        }
        assert_eq!(counts, [12, 48, 864, 6]);
        Ok(())
    }
}
