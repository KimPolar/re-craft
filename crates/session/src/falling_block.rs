//! Airborne movement for a falling block. World collision, landing and block
//! effects must be handled before this can drive a live entity.

use glam::{DVec3, IVec3};
use native_world::movement::BoundingBox;

/// Only use this fallback for collision shapes supplied by the native block
/// registry. Known exceptions such as snow and active fire tags must be
/// checked first; the remaining supported 26.3 shapes are checked against
/// FallingBlock.isFree.
pub fn known_collision_support(shape: &[native_world::collision::Aabb]) -> bool {
    !shape.is_empty()
}

/// Server-side expiry after an airborne tick. `max_y` is exclusive.
pub fn expires_in_air(age: i32, block_y: i32, min_y: i32, max_y: i32) -> bool {
    age > 600 || (age > 100 && (block_y <= min_y || block_y >= max_y))
}

/// The known subset of FallingBlock.isFree. Unknown states require the active
/// block registry's replaceability and fire tags before they can be decided.
pub fn known_free_below(block: &str, is_fire: bool) -> Option<bool> {
    if is_fire {
        return Some(true);
    }
    if let Some(id) = block.strip_prefix("minecraft:")
        && (id.ends_with("_sapling") || id.ends_with("_tulip"))
    {
        return Some(false);
    }
    match block {
        "minecraft:air"
        | "minecraft:cave_air"
        | "minecraft:void_air"
        | "minecraft:water"
        | "minecraft:lava"
        | "minecraft:short_grass"
        | "minecraft:fern"
        | "minecraft:dead_bush" => Some(true),
        "minecraft:stone"
        | "minecraft:dirt"
        | "minecraft:grass_block"
        | "minecraft:sand"
        | "minecraft:red_sand"
        | "minecraft:gravel"
        | "minecraft:bedrock"
        | "minecraft:cobblestone"
        | "minecraft:kelp"
        | "minecraft:kelp_plant"
        | "minecraft:dandelion"
        | "minecraft:poppy"
        | "minecraft:blue_orchid"
        | "minecraft:allium"
        | "minecraft:azure_bluet"
        | "minecraft:oxeye_daisy"
        | "minecraft:cornflower"
        | "minecraft:lily_of_the_valley"
        | "minecraft:torch"
        | "minecraft:wall_torch"
        | "minecraft:soul_torch"
        | "minecraft:soul_wall_torch" => Some(false),
        _ => None,
    }
}

/// `FallingBlock.isFree` for state-dependent snow and the known block subset.
pub fn known_falling_free(state: &native_world::loaded::BlockState, is_fire: bool) -> Option<bool> {
    if is_fire {
        return Some(true);
    }
    if state.name == "minecraft:snow" {
        return native_world::collision::snow_layers(state).map(|_| true);
    }
    known_free_below(&state.name, false)
}

/// Replacement with the empty-handed downward placement context used by a
/// falling block. Registry states not covered here require their own rules.
pub fn known_landing_replaceable(state: &native_world::loaded::BlockState) -> Option<bool> {
    if state.name == "minecraft:snow" {
        return native_world::collision::snow_layers(state).map(|layers| layers == 1);
    }
    match state.name.as_str() {
        "minecraft:fire" | "minecraft:soul_fire" => Some(true),
        block => known_free_below(block, false),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Collision {
    pub actual: DVec3,
    pub on_ground: bool,
    pub horizontal: bool,
    pub vertical: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum FallingKind {
    Sand,
    RedSand,
    Gravel,
    PointedDripstone,
    SulfurSpike,
}

/// FallingBlockEntity's saved damage parameters. The caller must separately
/// check whether the entity is configured to hurt entities.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallingDamage {
    pub per_distance: f32,
    pub maximum: i32,
}

impl FallingDamage {
    /// Java configures only the tip of a falling stalactite. Coordinates are
    /// absolute block positions and `top_y` must not be below `tip_y`.
    pub fn stalactite(top_y: i32, tip_y: i32) -> Option<Self> {
        let length = top_y.checked_sub(tip_y)?.checked_add(1)?;
        (length > 0).then_some(Self {
            per_distance: length.max(6) as f32,
            maximum: 40,
        })
    }

    /// `None` means Java does not call `hurt`; `Some(0)` still calls it with
    /// zero damage. Float multiplication is deliberately performed in f32.
    pub fn amount(self, fall_distance: f64) -> Option<i32> {
        let steps = (fall_distance - 1.0).ceil() as i32;
        if steps < 0 {
            return None;
        }
        let damage = ((steps as f32) * self.per_distance).floor() as i32;
        Some(damage.min(self.maximum))
    }
}

/// Block state carried by one falling stalactite segment. The source cell may
/// retain water, but FallingBlockEntity.fall always clears the carried state.
#[derive(Clone, Debug, PartialEq)]
pub struct FallingStalactite {
    block: native_world::loaded::BlockState,
    source_waterlogged: bool,
    damage: Option<FallingDamage>,
}

impl FallingStalactite {
    pub fn from_source(
        source: &native_world::loaded::BlockState,
        top_y: i32,
        segment_y: i32,
    ) -> Option<Self> {
        native_world::collision::positioned_speleothem_shape(source, IVec3::ZERO)?;
        if source.properties.get("vertical_direction")?.as_str() != "down" || segment_y > top_y {
            return None;
        }
        let source_waterlogged = source.properties.get("waterlogged")? == "true";
        let tip = matches!(
            source.properties.get("thickness")?.as_str(),
            "tip" | "tip_merge"
        );
        let damage = if tip {
            Some(FallingDamage::stalactite(top_y, segment_y)?)
        } else {
            None
        };
        let mut block = source.clone();
        block
            .properties
            .insert("waterlogged".into(), "false".into());
        Some(Self {
            block,
            source_waterlogged,
            damage,
        })
    }

    pub fn block(&self) -> &native_world::loaded::BlockState {
        &self.block
    }
    pub fn source_waterlogged(&self) -> bool {
        self.source_waterlogged
    }
    pub fn damage(&self) -> Option<FallingDamage> {
        self.damage
    }
}

impl FallingKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Sand => "minecraft:sand",
            Self::RedSand => "minecraft:red_sand",
            Self::Gravel => "minecraft:gravel",
            Self::PointedDripstone => "minecraft:pointed_dripstone",
            Self::SulfurSpike => "minecraft:sulfur_spike",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.strip_prefix("minecraft:").unwrap_or(id) {
            "sand" => Some(Self::Sand),
            "red_sand" => Some(Self::RedSand),
            "gravel" => Some(Self::Gravel),
            _ => None,
        }
    }
}

/// Immutable visual state for the renderer; positions are absolute world coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FallingBlockVisual {
    pub previous_position: DVec3,
    pub position: DVec3,
    pub kind: FallingKind,
}

impl FallingBlockVisual {
    /// Model origin relative to the render origin, preserving precision for far worlds.
    pub fn model_origin(&self, origin: IVec3, partial_tick: f32) -> DVec3 {
        let partial = f64::from(partial_tick);
        self.previous_position + (self.position - self.previous_position) * partial
            - origin.as_dvec3()
            + DVec3::new(-0.5, 0.0, -0.5)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirborneMotion {
    pub position: DVec3,
    pub velocity: DVec3,
    pub age: i32,
    pub no_gravity: bool,
}

/// Entity.checkFallDamage on an ordinary, dry movement path. Water and blocks
/// with special fall hooks must be handled separately by the caller.
pub fn fall_distance_after_move(distance: f64, actual_y: f64, on_ground: bool) -> f64 {
    if on_ground {
        0.0
    } else if actual_y < 0.0 {
        distance - f64::from(actual_y as f32)
    } else {
        distance
    }
}

impl AirborneMotion {
    /// Java creates the falling entity at the horizontal center of the block.
    pub fn from_block(position: IVec3) -> Self {
        Self {
            position: position.as_dvec3() + DVec3::new(0.5, 0.0, 0.5),
            velocity: DVec3::ZERO,
            age: 0,
            no_gravity: false,
        }
    }

    pub fn bounds(&self) -> BoundingBox {
        let half = f64::from(0.98_f32) / 2.0;
        BoundingBox {
            min: self.position + DVec3::new(-half, 0.0, -half),
            max: self.position + DVec3::new(half, f64::from(0.98_f32), half),
        }
    }
    /// Advance an unobstructed tick, excluding world and portal effects.
    pub fn tick_in_air(&mut self) {
        self.tick_with_collision(|_, requested| requested);
    }

    /// Move against a caller-supplied collision resolver for ordinary,
    /// non-bouncing surfaces. Landing and block effects are handled separately.
    pub fn tick_with_collision(&mut self, resolve: impl FnMut(DVec3, DVec3) -> DVec3) -> Collision {
        self.tick_with_collision_and_effects(resolve, |_, _, _| {})
    }

    /// Apply inside-block velocity effects after collision and before air drag.
    pub fn tick_with_collision_and_effects(
        &mut self,
        mut resolve: impl FnMut(DVec3, DVec3) -> DVec3,
        mut effects: impl FnMut(&mut DVec3, DVec3, DVec3),
    ) -> Collision {
        self.age = self.age.wrapping_add(1);
        if !self.no_gravity {
            self.velocity.y -= 0.04;
        }
        let requested = self.velocity;
        let actual = resolve(self.position, requested);
        self.position += actual;
        let hit_x = (requested.x - actual.x).abs() >= f64::from(1.0e-5_f32);
        let hit_z = (requested.z - actual.z).abs() >= f64::from(1.0e-5_f32);
        let hit_y = requested.y != actual.y;
        if hit_x {
            self.velocity.x = 0.0;
        }
        if hit_y {
            self.velocity.y = 0.0;
        }
        if hit_z {
            self.velocity.z = 0.0;
        }
        effects(&mut self.velocity, requested, actual);
        self.velocity *= f64::from(0.98_f32);
        Collision {
            actual,
            on_ground: hit_y && requested.y < 0.0,
            horizontal: hit_x || hit_z,
            vertical: hit_y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{IVec3, Vec3};
    use native_world::collision::Aabb;

    #[test]
    fn stalactite_damage_uses_height_threshold_rounding_and_cap() {
        let short = FallingDamage::stalactite(20, 19).unwrap();
        assert_eq!(short.per_distance, 6.0);
        assert_eq!(short.amount(0.0), None);
        assert_eq!(short.amount(0.01), Some(0));
        assert_eq!(short.amount(1.0), Some(0));
        assert_eq!(short.amount(1.000001), Some(6));
        assert_eq!(short.amount(2.0), Some(6));
        assert_eq!(short.amount(2.000001), Some(12));
        assert_eq!(short.amount(100.0), Some(40));
        assert_eq!(
            FallingDamage::stalactite(20, 14).unwrap().amount(1.1),
            Some(7)
        );
        assert!(FallingDamage::stalactite(19, 20).is_none());
        assert!(FallingDamage::stalactite(i32::MAX, i32::MIN).is_none());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn stalactite_damage_matches_java_falling_entity() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        assert_eq!(metadata["id"], "26.3");
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon"))
            .join("bin");
        let source = temp.path().join("FallingStalactiteDamageOracle.java");
        let output = temp.path().join("damage.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FallingStalactiteDamageOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("FallingStalactiteDamageOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 192);
        for row in rows {
            let length = row["length"].as_i64().unwrap() as i32;
            let distance = row["distance"].as_f64().unwrap();
            let damage = FallingDamage::stalactite(100, 101 - length)
                .unwrap()
                .amount(distance);
            assert_eq!(damage.is_some(), row["calls"].as_i64() == Some(1), "{row}");
            if let Some(damage) = damage {
                assert_eq!(
                    damage as f32,
                    row["amount"].as_f64().unwrap() as f32,
                    "{row}"
                );
            }
        }
    }

    #[test]
    fn inside_effect_is_applied_after_collision_before_drag() {
        let mut motion = AirborneMotion {
            position: DVec3::new(8.5, 10.1, 8.5),
            velocity: DVec3::new(0.2, -1.0, -0.3),
            age: 0,
            no_gravity: false,
        };
        let collision = motion.tick_with_collision_and_effects(
            |position, requested| DVec3::new(requested.x, 10.0 - position.y, requested.z),
            |velocity, requested, actual| {
                assert_eq!(requested.y, -1.04);
                assert!(actual.y > requested.y);
                assert_eq!(velocity.y, 0.0);
                *velocity = crate::bubble_column::player_velocity(*velocity, false, true, false);
            },
        );
        assert!(collision.on_ground);
        assert_eq!(motion.velocity.y, 0.1 * f64::from(0.98_f32));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn falling_bubble_motion_matches_java_tick() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in metadata["libraries"].as_array().unwrap() {
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
        let source = temp.path().join("FallingBubbleMotionOracle.java");
        let output = temp.path().join("falling.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/FallingBubbleMotionOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("FallingBubbleMotionOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.len(), 112);
        for row in rows {
            let mut motion = AirborneMotion {
                position: DVec3::new(8.5, 10.1, 8.5),
                velocity: DVec3::new(0.2, row["initial"].as_f64().unwrap(), -0.3),
                age: 0,
                no_gravity: row["noGravity"].as_bool().unwrap(),
            };
            let floor = row["floor"].as_bool().unwrap();
            let mut calls = 0;
            let collision = motion.tick_with_collision_and_effects(
                |position, requested| {
                    let y = if floor && position.y + requested.y < 10.0 {
                        10.0 - position.y
                    } else {
                        requested.y
                    };
                    DVec3::new(requested.x, y, requested.z)
                },
                |velocity, _, _| {
                    calls += 1;
                    *velocity = crate::bubble_column::player_velocity(
                        *velocity,
                        row["drag"].as_bool().unwrap(),
                        row["surface"].as_bool().unwrap(),
                        false,
                    );
                },
            );
            let expected = |name: &str| row[name].as_f64().unwrap();
            assert!(
                (motion.position - DVec3::new(expected("x"), expected("y"), expected("z")))
                    .abs()
                    .max_element()
                    < 1.0e-12,
                "{row}"
            );
            assert!(
                (motion.velocity - DVec3::new(expected("vx"), expected("vy"), expected("vz")))
                    .abs()
                    .max_element()
                    < 1.0e-12,
                "{row}"
            );
            assert_eq!(
                collision.on_ground,
                row["ground"].as_bool().unwrap(),
                "{row}"
            );
            assert_eq!(calls, row["calls"].as_u64().unwrap(), "{row}");
        }
    }

    #[test]
    fn fire_tag_can_make_a_solid_or_unknown_block_free() {
        assert_eq!(known_free_below("minecraft:stone", false), Some(false));
        assert_eq!(known_free_below("minecraft:stone", true), Some(true));
        assert_eq!(known_free_below("example:unknown", false), None);
        assert_eq!(known_free_below("example:unknown", true), Some(true));
    }

    #[test]
    fn spawn_uses_absolute_block_center_without_losing_large_coordinates() {
        for pos in [IVec3::new(i32::MIN, -64, i32::MAX), IVec3::new(-1, 64, -1)] {
            let motion = AirborneMotion::from_block(pos);
            assert_eq!(motion.position, pos.as_dvec3() + DVec3::new(0.5, 0.0, 0.5));
            assert_eq!(motion.velocity, DVec3::ZERO);
            assert_eq!(motion.age, 0);
        }
    }

    #[test]
    fn clipped_motion_stops_at_a_floor_and_wall() {
        const FULL: &[Aabb] = &[Aabb {
            min: Vec3::ZERO,
            max: Vec3::ONE,
        }];
        let mut motion = AirborneMotion {
            position: DVec3::new(0.5, 3.0, 0.5),
            velocity: DVec3::new(3.0, -4.0, 0.0),
            age: 0,
            no_gravity: false,
        };
        let half = f64::from(0.98_f32) / 2.0;
        let collision = motion.tick_with_collision(|position, requested| {
            let bounds = AirborneMotion {
                position,
                velocity: DVec3::ZERO,
                age: 0,
                no_gravity: false,
            }
            .bounds();
            crate::movement::resolve_box(bounds, requested, false, 0.0, |p: IVec3| {
                if p.y == 0 || (p.x == 2 && p.y >= 1 && p.y <= 4) {
                    FULL
                } else {
                    &[]
                }
            })
        });
        assert!(collision.on_ground && collision.horizontal && collision.vertical);
        assert!((motion.position.y - 1.0).abs() < 1.0e-9);
        assert!((motion.position.x - (2.0 - half)).abs() < 1.0e-9);
        assert_eq!(motion.velocity, DVec3::ZERO);
    }
}
