//! Placement geometry and free space used by Java's jigsaw assembler.
//!
//! Pool traversal, height sampling and template placement are separate stages.
use std::collections::{BTreeMap, VecDeque};

use anyhow::{Context, Result, ensure};
use native_core::beardifier::BoundingBox;

use crate::{jigsaw::Connector, structure_pool::Projection};

/// An integer-aligned box minus full piece boxes. Java tests a candidate after
/// deflating its box by 0.25 blocks before subtracting the full accepted box.
/// For integer-aligned boxes this is equivalent to inclusive block intersection.
#[derive(Clone, Debug)]
pub struct FreeSpace {
    bounds: BoundingBox,
    occupied: Vec<BoundingBox>,
}

impl FreeSpace {
    pub fn new(bounds: BoundingBox) -> Result<Self> {
        validate_box(bounds)?;
        Ok(Self {
            bounds,
            occupied: Vec::new(),
        })
    }

    /// Subtract a box even when it is only partly inside the outer bounds.
    pub fn subtract(&mut self, occupied: BoundingBox) -> Result<()> {
        validate_box(occupied)?;
        self.occupied.push(occupied);
        Ok(())
    }

    pub fn can_place(&self, candidate: BoundingBox) -> Result<bool> {
        validate_box(candidate)?;
        Ok((0..3).all(|i| {
            candidate.min[i] >= self.bounds.min[i] && candidate.max[i] <= self.bounds.max[i]
        }) && self.occupied.iter().all(|other| {
            (0..3).any(|i| candidate.max[i] < other.min[i] || candidate.min[i] > other.max[i])
        }))
    }

    /// Reserve only successful candidates; failed candidates leave the space intact.
    pub fn place(&mut self, candidate: BoundingBox) -> Result<bool> {
        if !self.can_place(candidate)? {
            return Ok(false);
        }
        self.occupied.push(candidate);
        Ok(true)
    }
}

fn validate_box(bounds: BoundingBox) -> Result<()> {
    ensure!(
        (0..3).all(|i| bounds.min[i] <= bounds.max[i]),
        "invalid jigsaw bounds"
    );
    // AABB.of performs max + 1 in Java i32 before converting to double. At
    // i32::MAX that wraps and is not an integer-aligned inclusive box anymore.
    ensure!(
        bounds.max.iter().all(|&value| value != i32::MAX),
        "jigsaw bounds exceed supported AABB coordinates"
    );
    Ok(())
}

/// Java's highest-priority-first queue retains insertion order within a level.
/// Its exhausted-queue scan excludes i32::MIN, matching 26.3's iterator.
pub struct PlacementQueue<T> {
    levels: BTreeMap<i32, VecDeque<T>>,
    highest: i32,
    current: Option<i32>,
    finished: bool,
}

impl<T> Default for PlacementQueue<T> {
    fn default() -> Self {
        Self {
            levels: BTreeMap::new(),
            highest: i32::MIN,
            current: None,
            finished: false,
        }
    }
}

impl<T> PlacementQueue<T> {
    pub fn push(&mut self, item: T, priority: i32) {
        self.levels.entry(priority).or_default().push_back(item);
        if self.current.is_none() || priority >= self.highest {
            self.highest = priority;
            self.current = Some(priority);
        }
    }
}

impl<T> Iterator for PlacementQueue<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.finished {
            return None;
        }
        let Some(priority) = self.current else {
            self.finished = true;
            return None;
        };
        let queue = self.levels.get_mut(&priority).expect("active queue exists");
        let result = queue.pop_front();
        if queue.is_empty() {
            self.current = self.levels.iter().rev().find_map(|(&priority, queue)| {
                (priority > i32::MIN && !queue.is_empty()).then_some(priority)
            });
            self.highest = self.current.unwrap_or(i32::MIN);
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Junction {
    pub source_x: i32,
    pub source_ground_y: i32,
    pub source_z: i32,
    pub delta_y: i32,
    pub destination_projection: Projection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttachedGeometry {
    pub origin: [i32; 3],
    pub bounds: BoundingBox,
    pub ground_level_delta: i32,
    pub source_junction: Junction,
    pub target_junction: Junction,
}

/// Inputs are already rotated: the target connector and target bounds are at
/// origin zero, while the source connector and source bounds are world-space.
pub struct Attachment<'a> {
    pub source: &'a Connector,
    pub target: &'a Connector,
    pub source_bounds: BoundingBox,
    pub target_bounds_at_origin: BoundingBox,
    pub source_projection: Projection,
    pub target_projection: Projection,
    pub source_ground_level_delta: i32,
    pub target_ground_level_delta: i32,
    /// WORLD_SURFACE_WG first-free height at the source connector. Required
    /// when either piece uses terrain matching.
    pub surface_height: Option<i32>,
    /// Maximum child pool height selected by Java's optional expansion hack.
    pub expansion: i32,
}

impl Attachment<'_> {
    pub fn geometry(&self) -> Result<Option<AttachedGeometry>> {
        if !self.source.can_attach(self.target) {
            return Ok(None);
        }
        validate_box(self.source_bounds)?;
        validate_box(self.target_bounds_at_origin)?;
        ensure!(self.expansion >= 0, "negative jigsaw expansion");
        let target_pos = self.source.adjacent_position();
        let mut origin =
            std::array::from_fn(|i| target_pos[i].wrapping_sub(self.target.position[i]));
        let source_local_y = self.source.position[1].wrapping_sub(self.source_bounds.min[1]);
        let target_local_y = self.target.position[1];
        let delta_y = source_local_y
            .wrapping_sub(target_local_y)
            .wrapping_add(self.source.orientation.front.step()[1]);
        let source_rigid = self.source_projection == Projection::Rigid;
        let target_rigid = self.target_projection == Projection::Rigid;
        let target_y = if source_rigid && target_rigid {
            self.source_bounds.min[1].wrapping_add(delta_y)
        } else {
            self.surface_height
                .context("jigsaw surface height is missing")?
                .wrapping_sub(target_local_y)
        };
        let raw_min_y = self.target_bounds_at_origin.min[1].wrapping_add(origin[1]);
        let offset_y = target_y.wrapping_sub(raw_min_y);
        origin[1] = origin[1].wrapping_add(offset_y);
        let min =
            std::array::from_fn(|i| self.target_bounds_at_origin.min[i].wrapping_add(origin[i]));
        let mut max =
            std::array::from_fn(|i| self.target_bounds_at_origin.max[i].wrapping_add(origin[i]));
        if self.expansion > 0 {
            let new_size = self
                .expansion
                .wrapping_add(1)
                .max(max[1].wrapping_sub(min[1]));
            max[1] = max[1].max(min[1].wrapping_add(new_size));
        }
        let bounds = BoundingBox::new(min, max).map_err(anyhow::Error::msg)?;
        let ground_level_delta = if target_rigid {
            self.source_ground_level_delta.wrapping_sub(delta_y)
        } else {
            self.target_ground_level_delta
        };
        let junction_y = if source_rigid {
            self.source_bounds.min[1].wrapping_add(source_local_y)
        } else if target_rigid {
            target_y.wrapping_add(target_local_y)
        } else {
            self.surface_height
                .context("jigsaw surface height is missing")?
                .wrapping_add(delta_y / 2)
        };
        let source_junction = Junction {
            source_x: target_pos[0],
            source_ground_y: junction_y
                .wrapping_sub(source_local_y)
                .wrapping_add(self.source_ground_level_delta),
            source_z: target_pos[2],
            delta_y,
            destination_projection: self.target_projection,
        };
        let target_junction = Junction {
            source_x: self.source.position[0],
            source_ground_y: junction_y
                .wrapping_sub(target_local_y)
                .wrapping_add(ground_level_delta),
            source_z: self.source.position[2],
            delta_y: delta_y.wrapping_neg(),
            destination_projection: self.source_projection,
        };
        Ok(Some(AttachedGeometry {
            origin,
            bounds,
            ground_level_delta,
            source_junction,
            target_junction,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jigsaw::{Joint, Orientation},
        packs::ResourceId,
        structure_transform::{self, Mirror, Rotation},
    };

    fn bounds(min: [i32; 3], max: [i32; 3]) -> BoundingBox {
        BoundingBox::new(min, max).unwrap()
    }
    fn connector(position: [i32; 3], orientation: &str, name: &str) -> Connector {
        Connector {
            position,
            orientation: Orientation::parse(orientation).unwrap(),
            joint: Joint::Aligned,
            name: Some(ResourceId::parse(name).unwrap()),
            target: ResourceId::parse("example:door").unwrap(),
            pool: ResourceId::parse("minecraft:empty").unwrap(),
            placement_priority: 0,
            selection_priority: 0,
        }
    }

    #[test]
    fn free_space_and_priority_are_transactional() -> Result<()> {
        let mut space = FreeSpace::new(bounds([-5; 3], [5; 3]))?;
        space.subtract(bounds([0; 3], [1; 3]))?;
        assert!(!space.place(bounds([1; 3], [2; 3]))?);
        assert!(space.place(bounds([2; 3], [3; 3]))?);
        assert!(!space.place(bounds([3; 3], [4; 3]))?);
        assert!(!space.place(bounds([5; 3], [6; 3]))?);
        assert!(FreeSpace::new(bounds([0; 3], [i32::MAX; 3])).is_err());
        let mut queue = PlacementQueue::default();
        queue.push('a', 2);
        queue.push('b', 2);
        queue.push('c', 5);
        assert_eq!(queue.next(), Some('c'));
        queue.push('d', 3);
        assert_eq!(queue.collect::<String>(), "dab");
        Ok(())
    }

    #[test]
    fn terrain_matching_requires_height_and_preserves_junctions() -> Result<()> {
        let source = connector([4, 12, 2], "east_up", "example:parent");
        let target = connector([0, 1, 2], "west_up", "example:door");
        let mut input = Attachment {
            source: &source,
            target: &target,
            source_bounds: bounds([0, 10, 0], [4, 14, 4]),
            target_bounds_at_origin: bounds([0, 0, 0], [3, 2, 4]),
            source_projection: Projection::TerrainMatching,
            target_projection: Projection::TerrainMatching,
            source_ground_level_delta: 1,
            target_ground_level_delta: 2,
            surface_height: None,
            expansion: 0,
        };
        assert!(input.geometry().is_err());
        input.surface_height = Some(70);
        let result = input.geometry()?.unwrap();
        assert_eq!(result.origin, [5, 69, 0]);
        assert_eq!(result.ground_level_delta, 2);
        assert_eq!(result.source_junction.source_ground_y, 69);
        assert_eq!(result.target_junction.source_ground_y, 71);
        input.expansion = 5;
        assert_eq!(input.geometry()?.unwrap().bounds.max[1], 75);
        Ok(())
    }

    #[test]
    #[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
    fn placement_space_queue_and_geometry_match_java() -> Result<()> {
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
        let source = temp.path().join("JigsawPlacementOracle.java");
        std::fs::write(
            &source,
            include_str!("../tests/java/JigsawPlacementOracle.java"),
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
            .arg("JigsawPlacementOracle")
            .output()?;
        ensure!(
            output.status.success(),
            "Java oracle failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut space = FreeSpace::new(bounds([-5; 3], [5; 3]))?;
        space.subtract(bounds([-1; 3], [1; 3]))?;
        let mut seen = [0usize; 3];
        for line in String::from_utf8(output.stdout)?.lines() {
            let fields: Vec<_> = line.split('\t').collect();
            match fields.as_slice() {
                ["B", x, y, z, sx, sy, sz, accepted] => {
                    let min = [x.parse::<i32>()?, y.parse::<i32>()?, z.parse::<i32>()?];
                    let spans = [sx.parse::<i32>()?, sy.parse::<i32>()?, sz.parse::<i32>()?];
                    let max = std::array::from_fn(|i| min[i] + spans[i]);
                    assert_eq!(space.place(bounds(min, max))?, accepted.parse::<bool>()?);
                    seen[0] += 1;
                }
                ["Q", expected] => {
                    let mut queue = PlacementQueue::default();
                    queue.push('a', i32::MIN);
                    queue.push('b', 2);
                    queue.push('c', 2);
                    let mut order = String::new();
                    order.push(queue.next().context("missing first queue item")?);
                    queue.push('d', 3);
                    queue.push('e', 2);
                    queue.push('f', -1);
                    order.extend(queue);
                    assert_eq!(order, *expected);
                    seen[1] += 1;
                }
                [
                    "G",
                    mode,
                    projection,
                    height_case,
                    _seed,
                    rotation,
                    px,
                    py,
                    pz,
                    x0,
                    y0,
                    z0,
                    x1,
                    y1,
                    z1,
                    ground,
                    jx,
                    jy,
                    jz,
                    jd,
                    kx,
                    ky,
                    kz,
                    kd,
                    _next,
                    source_count,
                    target_count,
                ] => {
                    let mode = mode.parse::<usize>()?;
                    let projection = projection.parse::<usize>()?;
                    let height_case = height_case.parse::<usize>()?;
                    let rotations = [
                        Rotation::None,
                        Rotation::Clockwise90,
                        Rotation::Clockwise180,
                        Rotation::Counterclockwise90,
                    ];
                    let rotation = rotations[rotation.parse::<usize>()?];
                    let positions = [
                        [0, 1, 2],
                        [1, 1, 0],
                        [3, 1, 2],
                        [1, 1, 4],
                        [1, 0, 2],
                        [1, 2, 2],
                    ];
                    let orientations = [
                        "west_up",
                        "north_up",
                        "east_up",
                        "south_up",
                        "down_north",
                        "up_north",
                    ];
                    let (source_pos, source_orientation, height) = match mode {
                        0..=3 => ([14, 40 + [2, 0, 4][height_case], 22], "east_up", 42),
                        4 => ([12, 44, 22], "up_north", 45),
                        _ => ([12, 40, 22], "down_north", 39),
                    };
                    let source = connector(source_pos, source_orientation, "example:parent");
                    let target = connector(positions[mode], orientations[mode], "example:door")
                        .transformed([0; 3], rotation);
                    let target_bounds = structure_transform::bounding_box(
                        [4, 3, 5],
                        [0; 3],
                        Mirror::None,
                        rotation,
                        [0; 3],
                    )?;
                    let attachment = Attachment {
                        source: &source,
                        target: &target,
                        source_bounds: bounds([10, 40, 20], [14, 44, 24]),
                        target_bounds_at_origin: target_bounds,
                        source_projection: if projection & 1 == 0 {
                            Projection::Rigid
                        } else {
                            Projection::TerrainMatching
                        },
                        target_projection: if projection & 2 == 0 {
                            Projection::Rigid
                        } else {
                            Projection::TerrainMatching
                        },
                        source_ground_level_delta: 1,
                        target_ground_level_delta: 1,
                        surface_height: Some(height),
                        expansion: 0,
                    };
                    let result = attachment
                        .geometry()?
                        .context("Java placed a mismatched connector")?;
                    assert_eq!(
                        result.origin,
                        [px.parse::<i32>()?, py.parse::<i32>()?, pz.parse::<i32>()?]
                    );
                    assert_eq!(
                        result.bounds,
                        bounds(
                            [x0.parse()?, y0.parse()?, z0.parse()?],
                            [x1.parse()?, y1.parse()?, z1.parse()?]
                        )
                    );
                    assert_eq!(result.ground_level_delta, ground.parse::<i32>()?);
                    assert_eq!(
                        [
                            result.source_junction.source_x,
                            result.source_junction.source_ground_y,
                            result.source_junction.source_z,
                            result.source_junction.delta_y
                        ],
                        [
                            jx.parse::<i32>()?,
                            jy.parse::<i32>()?,
                            jz.parse::<i32>()?,
                            jd.parse::<i32>()?
                        ]
                    );
                    assert_eq!(
                        [
                            result.target_junction.source_x,
                            result.target_junction.source_ground_y,
                            result.target_junction.source_z,
                            result.target_junction.delta_y
                        ],
                        [
                            kx.parse::<i32>()?,
                            ky.parse::<i32>()?,
                            kz.parse::<i32>()?,
                            kd.parse::<i32>()?
                        ]
                    );
                    assert_eq!((*source_count, *target_count), ("1", "1"));
                    seen[2] += 1;
                }
                _ => {}
            }
        }
        assert_eq!(seen, [240, 1, 432]);
        Ok(())
    }
}
