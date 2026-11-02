//! Java 26.3 game-event frequencies and vibration candidate selection.
//!
//! This module does not yet discover listeners or activate sculk sensors.

use glam::{DVec3, IVec3};

/// Frequency used by vanilla's vibration system. Unregistered events have no
/// frequency; data-pack tags determine separately whether an event is heard.
pub fn frequency(id: &str) -> u8 {
    let path = if let Some(path) = id.strip_prefix("minecraft:") {
        path
    } else if !id.contains(':') {
        id
    } else {
        return 0;
    };
    match path {
        "step" | "swim" | "flap" | "resonate_1" => 1,
        "projectile_land" | "hit_ground" | "splash" | "bounce" | "resonate_2" => 2,
        "item_interact_finish" | "projectile_shoot" | "instrument_play" | "resonate_3" => 3,
        "entity_action" | "elytra_glide" | "unequip" | "resonate_4" => 4,
        "entity_dismount" | "equip" | "resonate_5" => 5,
        "entity_mount" | "entity_interact" | "shear" | "resonate_6" => 6,
        "entity_damage" | "resonate_7" => 7,
        "drink" | "eat" | "resonate_8" => 8,
        "container_close" | "block_close" | "block_deactivate" | "block_detach" | "resonate_9" => 9,
        "container_open" | "block_open" | "block_activate" | "block_attach" | "prime_fuse"
        | "note_block_play" | "resonate_10" => 10,
        "block_change" | "resonate_11" => 11,
        "block_destroy" | "fluid_pickup" | "resonate_12" => 12,
        "block_place" | "fluid_place" | "resonate_13" => 13,
        "entity_place" | "lightning_strike" | "teleport" | "resonate_14" => 14,
        "entity_die" | "explode" | "resonate_15" => 15,
        _ => 0,
    }
}

/// The receiver's redstone strength, including Java's float-to-double order.
pub fn redstone_strength(distance: f32, radius: i32) -> Option<i32> {
    if radius <= 0 || !distance.is_finite() {
        return None;
    }
    let decrement = ((15.0_f64 / f64::from(radius)) * f64::from(distance)).floor() as i32;
    Some(15_i32.wrapping_sub(decrement).max(1))
}

/// The travel delay uses the precise event and listener positions.
pub fn travel_distance(event: DVec3, listener: DVec3) -> Option<f32> {
    if !event.is_finite() || !listener.is_finite() {
        return None;
    }
    let d = event - listener;
    let distance = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt() as f32;
    distance.is_finite().then_some(distance)
}

/// The signal uses the distance between the floored event and listener blocks.
pub fn block_distance(event: IVec3, listener: IVec3) -> f32 {
    let d = event.as_dvec3() - listener.as_dvec3();
    (d.x * d.x + d.y * d.y + d.z * d.z).sqrt() as f32
}

/// Java checks six rays between block centers, shifted at the source by a
/// small amount in each cardinal direction. `occludes` reads the active block
/// tag; None means the block or its tag is unavailable. A definite clear ray
/// or six definite blocked rays can still be resolved despite unknown cells.
pub fn occluded(
    event: DVec3,
    listener: DVec3,
    mut occludes: impl FnMut(IVec3) -> Option<bool>,
) -> Option<bool> {
    fn center(value: DVec3) -> Option<DVec3> {
        if !value.is_finite()
            || value.min_element() < f64::from(i32::MIN) + 1.0
            || value.max_element() >= f64::from(i32::MAX) - 1.0
        {
            return None;
        }
        Some(value.floor() + DVec3::splat(0.5))
    }
    let from = center(event)?;
    let to = center(listener)?;
    // Sensor radii are small; avoid unbounded walks for malformed input.
    if (from - to).abs().element_sum() > 4096.0 {
        return None;
    }
    let epsilon = f64::from(1.0e-5_f32);
    let mut uncertain = false;
    for offset in [
        DVec3::NEG_Y,
        DVec3::Y,
        DVec3::NEG_Z,
        DVec3::Z,
        DVec3::NEG_X,
        DVec3::X,
    ] {
        match occlusion_ray(from + offset * epsilon, to, &mut occludes) {
            Some(false) => return Some(false),
            Some(true) => {}
            None => uncertain = true,
        }
    }
    (!uncertain).then_some(true)
}

fn occlusion_ray(
    from: DVec3,
    to: DVec3,
    occludes: &mut impl FnMut(IVec3) -> Option<bool>,
) -> Option<bool> {
    if from == to {
        return Some(false);
    }
    // BlockGetter.traverseBlocks extends both endpoints by 1e-7.
    let end = to + (from - to) * -1.0e-7;
    let start = from + (to - from) * -1.0e-7;
    let delta = end - start;
    let mut cell = start.floor().as_ivec3();
    let mut unknown = false;
    let mut visit = |cell| match occludes(cell) {
        Some(true) => true,
        Some(false) => false,
        None => {
            unknown = true;
            false
        }
    };
    if visit(cell) {
        return Some(true);
    }
    let mut step = [0_i32; 3];
    let mut increment = [0.0; 3];
    let mut next = [0.0; 3];
    for axis in 0..3 {
        step[axis] = if delta[axis] > 0.0 {
            1
        } else if delta[axis] < 0.0 {
            -1
        } else {
            0
        };
        increment[axis] = if step[axis] == 0 {
            f64::MAX
        } else {
            f64::from(step[axis]) / delta[axis]
        };
        let fraction = start[axis] - start[axis].floor();
        next[axis] = increment[axis]
            * if step[axis] > 0 {
                1.0 - fraction
            } else {
                fraction
            };
    }
    while next.iter().any(|value| *value <= 1.0) {
        let axis = if next[0] < next[1] {
            if next[0] < next[2] { 0 } else { 2 }
        } else if next[1] < next[2] {
            1
        } else {
            2
        };
        cell[axis] = cell[axis].checked_add(step[axis])?;
        next[axis] += increment[axis];
        if visit(cell) {
            return Some(true);
        }
    }
    if unknown { None } else { Some(false) }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VibrationSource {
    pub spectator: bool,
    pub stepping_carefully: bool,
    pub dampens_vibrations: bool,
    pub server_player: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterResult {
    Accept,
    Reject,
    /// Java rejects the event and invokes the avoid-vibration advancement.
    AvoidVibration,
}

/// Default VibrationSystem.User filtering. The caller determines whether the
/// affected block belongs to the active dampening block tag.
pub fn filter(
    tags: &native_resources::game_event_tags::GameEventTags,
    event: &str,
    source: Option<VibrationSource>,
    affected_block_dampens: bool,
    can_trigger_avoid: bool,
) -> FilterResult {
    if !tags.vibrates(event) {
        return FilterResult::Reject;
    }
    if let Some(source) = source {
        if source.spectator {
            return FilterResult::Reject;
        }
        if source.stepping_carefully && tags.ignored_while_sneaking(event) {
            return if can_trigger_avoid && source.server_player {
                FilterResult::AvoidVibration
            } else {
                FilterResult::Reject
            };
        }
        if source.dampens_vibrations {
            return FilterResult::Reject;
        }
    }
    if affected_block_dampens {
        FilterResult::Reject
    } else {
        FilterResult::Accept
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Vibration {
    pub event: String,
    pub distance: f32,
    pub position: DVec3,
    pub source: Option<[i32; 4]>,
    pub projectile_owner: Option<[i32; 4]>,
}

/// Java considers candidates from the same tick only: closest wins, then
/// higher frequency. A candidate becomes eligible on a later tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VibrationSelector {
    pending: Option<(i64, Vibration)>,
}

impl VibrationSelector {
    pub fn offer(&mut self, candidate: Vibration, tick: i64) {
        let replace = match &self.pending {
            None => true,
            Some((old_tick, old)) if *old_tick == tick => {
                candidate.distance < old.distance
                    || (!(candidate.distance > old.distance)
                        && frequency(&candidate.event) > frequency(&old.event))
            }
            _ => false,
        };
        if replace {
            self.pending = Some((tick, candidate));
        }
    }

    pub fn chosen(&self, tick: i64) -> Option<&Vibration> {
        self.pending
            .as_ref()
            .filter(|(created, _)| *created < tick)
            .map(|(_, value)| value)
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }
}

/// Travel and selection state for a listener. Event filtering, occlusion,
/// particle packets and listener-specific effects remain the caller's job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VibrationData {
    selector: VibrationSelector,
    current: Option<Vibration>,
    remaining: i32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VibrationTick {
    pub selected: bool,
    pub arrived: Option<Vibration>,
    /// Number of Java `onDataChanged` calls in this tick.
    pub data_changes: u8,
}

impl VibrationData {
    pub(crate) fn from_saved(
        current: Option<Vibration>,
        remaining: i32,
        pending: Option<(i64, Vibration)>,
    ) -> Self {
        Self {
            selector: VibrationSelector { pending },
            current,
            remaining,
        }
    }

    pub fn pending(&self) -> Option<(i64, &Vibration)> {
        self.selector
            .pending
            .as_ref()
            .map(|(tick, event)| (*tick, event))
    }

    pub fn offer(&mut self, candidate: Vibration, tick: i64) -> bool {
        if self.current.is_some() || !candidate.distance.is_finite() || candidate.distance < 0.0 {
            return false;
        }
        self.selector.offer(candidate, tick);
        true
    }

    pub fn current(&self) -> Option<&Vibration> {
        self.current.as_ref()
    }
    pub fn remaining(&self) -> i32 {
        self.remaining
    }

    /// `ready` represents the listener's required neighboring chunks being
    /// available. A vibration waits at zero until delivery becomes possible.
    pub fn tick(&mut self, game_time: i64, ready: bool) -> VibrationTick {
        let mut result = VibrationTick::default();
        if self.current.is_none() {
            if let Some(candidate) = self.selector.chosen(game_time).cloned() {
                self.remaining = candidate.distance.floor() as i32;
                self.current = Some(candidate);
                self.selector.clear();
                result.selected = true;
                result.data_changes += 1;
            }
        }
        if self.current.is_some() {
            let was_travelling = self.remaining > 0;
            self.remaining = self.remaining.saturating_sub(1).max(0);
            if self.remaining == 0 {
                if ready {
                    result.arrived = self.current.take();
                    result.data_changes += 1;
                }
            } else if was_travelling {
                result.data_changes += 1;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occlusion_keeps_unknowns_unless_a_clear_ray_or_six_blocked_rays_prove_the_result() {
        let from = DVec3::new(0.5, 0.5, 0.5);
        let to = DVec3::new(3.5, 0.5, 0.5);
        assert_eq!(occluded(from, to, |_| Some(false)), Some(false));
        assert_eq!(occluded(from, to, |p| Some(p.x == 2)), Some(true));
        assert_eq!(
            occluded(from, to, |p| if p.x == 1 { None } else { Some(false) }),
            None
        );
        assert_eq!(
            occluded(from, to, |p| if p.x == 1 { None } else { Some(p.x == 2) }),
            Some(true)
        );
        assert_eq!(occluded(DVec3::splat(f64::NAN), to, |_| Some(false)), None);
        assert_eq!(occluded(from, DVec3::splat(5000.0), |_| Some(false)), None);
    }

    #[test]
    fn selector_waits_and_orders_distance_then_frequency() {
        let event = |name: &str, distance: f32| Vibration {
            event: name.into(),
            distance,
            position: DVec3::ZERO,
            source: None,
            projectile_owner: None,
        };
        let mut selector = VibrationSelector::default();
        selector.offer(event("minecraft:splash", 4.0), 7);
        selector.offer(event("minecraft:explode", 5.0), 7);
        assert!(selector.chosen(7).is_none());
        assert_eq!(selector.chosen(8).unwrap().event, "minecraft:splash");
        selector.offer(event("minecraft:step", 3.0), 7);
        selector.offer(event("minecraft:block_place", 3.0), 7);
        selector.offer(event("minecraft:explode", 0.0), 8);
        assert_eq!(selector.chosen(8).unwrap().event, "minecraft:block_place");
        selector.clear();
        assert!(selector.chosen(100).is_none());
    }

    #[test]
    fn delivery_waits_for_required_chunks_and_rejects_new_events_in_transit() {
        let event = Vibration {
            event: "minecraft:splash".into(),
            distance: 2.9,
            position: DVec3::ZERO,
            source: None,
            projectile_owner: None,
        };
        let mut state = VibrationData::default();
        assert!(state.offer(event.clone(), 4));
        assert_eq!(state.tick(4, true), VibrationTick::default());
        let first = state.tick(5, false);
        assert!(first.selected);
        assert_eq!(first.data_changes, 2);
        assert_eq!(state.remaining(), 1);
        assert!(!state.offer(event.clone(), 5));
        assert!(state.tick(6, false).arrived.is_none());
        assert_eq!(state.remaining(), 0);
        assert_eq!(state.tick(7, false).data_changes, 0);
        assert_eq!(state.tick(8, true).arrived, Some(event));
        assert!(state.current().is_none());
    }

    #[test]
    fn careful_server_player_reports_avoidance_before_dampening() {
        use std::collections::BTreeSet;
        let tags = native_resources::game_event_tags::GameEventTags::new(
            BTreeSet::from(["minecraft:step".into()]),
            BTreeSet::from(["minecraft:step".into()]),
        );
        let source = VibrationSource {
            stepping_carefully: true,
            dampens_vibrations: true,
            server_player: true,
            ..Default::default()
        };
        assert_eq!(
            filter(&tags, "minecraft:step", Some(source), true, true),
            FilterResult::AvoidVibration
        );
        assert_eq!(
            filter(&tags, "minecraft:step", Some(source), true, false),
            FilterResult::Reject
        );
        assert_eq!(
            filter(&tags, "minecraft:splash", Some(source), false, true),
            FilterResult::Reject
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn frequencies_strength_and_selection_match_java() {
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
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
            .map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
                .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("VibrationOracle.java");
        let output = temp.path().join("output.json");
        std::fs::write(&source, include_str!("../tests/java/VibrationOracle.java")).unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("VibrationOracle")
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
        let data: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let frequencies = data["frequencies"].as_object().unwrap();
        assert_eq!(frequencies.len(), 61);
        let registry = frequencies.keys().cloned().collect();
        let sensors = native_persistence::nbt::read(
            &std::fs::read(temp.path().join("output.json.nbt")).unwrap(),
        )
        .unwrap();
        for n in 0..4 {
            use native_persistence::nbt::{NamedTag, Tag, TagType};
            let entity = sensors.value.get(&format!("sensor{n}")).unwrap().clone();
            let bytes = native_persistence::nbt::write(&NamedTag {
                name: "".into(),
                value: Tag::List {
                    element_type: TagType::Compound,
                    values: vec![entity.clone()],
                },
            })
            .unwrap();
            let saved = crate::saved_sculk::read(
                &native_world::loaded::SavedBlockEntities::Encoded(bytes.into()),
                native_world::loaded::ChunkPos { x: -1, z: 0 },
                IVec3::new(-1, 42, 3),
                &registry,
            )
            .unwrap()
            .unwrap();
            assert_eq!(saved.calibrated, n % 2 == 1);
            assert_eq!(saved.last_frequency, n + 1);
            assert!(saved.reload_vibration_particle);
            assert_eq!(saved.source(), &entity);
            if n == 1 || n == 3 {
                let (tick, pending) = saved.vibration.pending().unwrap();
                assert_eq!(
                    (
                        tick,
                        pending.event.as_str(),
                        pending.distance,
                        pending.position,
                        pending.source
                    ),
                    (
                        99,
                        "minecraft:splash",
                        1.25,
                        DVec3::new(-1.25, 42.5, 3.75),
                        Some([0, 1, 0, 2])
                    )
                );
            } else {
                assert!(saved.vibration.pending().is_none());
            }
            if n >= 2 {
                let current = saved.vibration.current().unwrap();
                assert_eq!(
                    (
                        current.event.as_str(),
                        current.distance,
                        current.position,
                        current.source,
                        current.projectile_owner,
                        saved.vibration.remaining()
                    ),
                    (
                        "minecraft:step",
                        4.5,
                        DVec3::new(2.25, 42.0, -3.5),
                        Some([0, 3, 0, 4]),
                        Some([0, 5, 0, 6]),
                        3
                    )
                );
            } else {
                assert!(saved.vibration.current().is_none());
            }
        }
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let tags =
            native_resources::game_event_tags::GameEventTags::load(&mut resources, &registry)
                .unwrap();
        assert!(tags.vibrates("minecraft:splash"));
        assert!(tags.ignored_while_sneaking("minecraft:step"));
        assert!(!tags.ignored_while_sneaking("minecraft:splash"));
        let block_registry = data["block_registry"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let block_tags = native_resources::game_event_tags::VibrationBlockTags::load(
            &mut resources,
            &block_registry,
        )
        .unwrap();
        assert_eq!(block_tags.occludes("minecraft:white_wool"), Some(true));
        assert_eq!(block_tags.occludes("minecraft:white_carpet"), Some(false));
        assert_eq!(block_tags.dampens("minecraft:white_carpet"), Some(true));
        assert_eq!(block_tags.resonates("minecraft:amethyst_block"), Some(true));
        for (event, value) in frequencies {
            assert_eq!(frequency(event), value.as_u64().unwrap() as u8, "{event}");
        }
        assert_eq!(frequency("example:splash"), 0);
        assert_eq!(frequency("minecraft:resonate_01"), 0);
        for row in data["strengths"].as_array().unwrap() {
            let distance = f32::from_bits(row["distance"].as_i64().unwrap() as u32);
            assert_eq!(
                redstone_strength(distance, row["radius"].as_i64().unwrap() as i32),
                Some(row["strength"].as_i64().unwrap() as i32),
                "{row}"
            );
        }
        for row in data["distances"].as_array().unwrap() {
            let values: Vec<f64> = row["coords"]
                .as_array()
                .unwrap()
                .iter()
                .map(|bits| f64::from_bits(bits.as_i64().unwrap() as u64))
                .collect();
            let a = DVec3::new(values[0], values[1], values[2]);
            let b = DVec3::new(values[3], values[4], values[5]);
            assert_eq!(
                travel_distance(a, b).unwrap().to_bits(),
                row["travel"].as_i64().unwrap() as u32,
                "{row}"
            );
            assert_eq!(
                block_distance(a.floor().as_ivec3(), b.floor().as_ivec3()).to_bits(),
                row["blocks"].as_i64().unwrap() as u32,
                "{row}"
            );
        }
        let rays = data["rays"].as_array().unwrap();
        assert_eq!(rays.len(), 1200);
        for row in rays {
            let shift = row["shift"].as_i64().unwrap() as f64;
            let blocks: std::collections::HashSet<IVec3> = row["blocks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    let n = v.as_array().unwrap();
                    IVec3::new(
                        n[0].as_i64().unwrap() as i32,
                        n[1].as_i64().unwrap() as i32,
                        n[2].as_i64().unwrap() as i32,
                    )
                })
                .collect();
            let to = DVec3::new(
                shift + row["x"].as_i64().unwrap() as f64 + 0.3,
                row["y"].as_i64().unwrap() as f64 + 0.8,
                shift + row["z"].as_i64().unwrap() as f64 + 0.9,
            );
            assert_eq!(
                occluded(DVec3::new(shift + 0.25, 0.75, shift + 0.1), to, |pos| Some(
                    blocks.contains(&pos)
                )),
                Some(row["occluded"].as_bool().unwrap()),
                "{row}"
            );
        }
        let selections = data["selections"].as_array().unwrap();
        assert_eq!(selections.len(), 61 * 61 * 5);
        for row in selections {
            let mut selector = VibrationSelector::default();
            for (name, distance, tick, marker) in [
                ("first", "d1", 5, 1.0),
                ("second", "d2", row["tick2"].as_i64().unwrap(), 2.0),
            ] {
                selector.offer(
                    Vibration {
                        event: row[name].as_str().unwrap().into(),
                        distance: row[distance].as_f64().unwrap() as f32,
                        position: DVec3::new(marker, 0.0, 0.0),
                        source: None,
                        projectile_owner: None,
                    },
                    tick,
                );
            }
            assert_eq!(
                selector.chosen(5).is_some(),
                row["same_tick"].as_bool().unwrap()
            );
            assert_eq!(
                selector.chosen(6).unwrap().position.x,
                row["chosen"].as_f64().unwrap(),
                "{row}"
            );
        }
        let timing = data["timing"].as_array().unwrap();
        assert_eq!(timing.len(), 154);
        for group in timing.chunks_exact(11) {
            let distance = f32::from_bits(group[0]["distance"].as_i64().unwrap() as u32);
            let event = Vibration {
                event: "minecraft:splash".into(),
                distance,
                position: DVec3::new(1.0, 0.0, 0.0),
                source: None,
                projectile_owner: None,
            };
            let mut state = VibrationData::default();
            assert!(state.offer(event, 100));
            let mut received = 0_u64;
            let mut changed = 0_u64;
            for row in group {
                let tick = state.tick(
                    row["tick"].as_i64().unwrap(),
                    row["ready"].as_bool().unwrap(),
                );
                received += u64::from(tick.arrived.is_some());
                changed += u64::from(tick.data_changes);
                assert_eq!(
                    state.current().is_some(),
                    row["active"].as_bool().unwrap(),
                    "{row}"
                );
                assert_eq!(
                    i64::from(state.remaining()),
                    row["remaining"].as_i64().unwrap(),
                    "{row}"
                );
                assert_eq!(received, row["received"].as_u64().unwrap(), "{row}");
                assert_eq!(changed, row["changed"].as_u64().unwrap(), "{row}");
            }
        }
        let filters = data["filter"].as_array().unwrap();
        assert_eq!(filters.len(), 128);
        for row in filters {
            let mask = row["mask"].as_u64().unwrap();
            let event = "minecraft:splash";
            let tags = native_resources::game_event_tags::GameEventTags::new(
                if mask & 1 != 0 {
                    [event.to_owned()].into()
                } else {
                    Default::default()
                },
                if mask & 2 != 0 {
                    [event.to_owned()].into()
                } else {
                    Default::default()
                },
            );
            let source = (mask & 4 != 0).then_some(VibrationSource {
                spectator: mask & 8 != 0,
                stepping_carefully: mask & 16 != 0,
                dampens_vibrations: mask & 32 != 0,
                server_player: false,
            });
            assert_eq!(
                filter(&tags, event, source, mask & 64 != 0, false) == FilterResult::Accept,
                row["accepted"].as_bool().unwrap(),
                "{row}"
            );
        }
        let parse_sensor = |row: &serde_json::Value| {
            let calibrated = row["calibrated"].as_bool().unwrap();
            let mut properties = std::collections::BTreeMap::from([
                (
                    "sculk_sensor_phase".to_owned(),
                    row["phase"].as_str().unwrap().to_owned(),
                ),
                (
                    "power".to_owned(),
                    row["power"].as_u64().unwrap_or(0).to_string(),
                ),
                (
                    "waterlogged".to_owned(),
                    row["wet"].as_bool().unwrap_or(false).to_string(),
                ),
            ]);
            if calibrated {
                properties.insert("facing".into(), "north".into());
            }
            crate::sculk_sensor::Sensor::from_block(&native_world::loaded::BlockState {
                name: if calibrated {
                    "minecraft:calibrated_sculk_sensor"
                } else {
                    "minecraft:sculk_sensor"
                }
                .into(),
                properties,
            })
            .unwrap()
        };
        let signals = data["sensor_signals"].as_array().unwrap();
        assert_eq!(signals.len(), 2 * 3 * 16 * 2 * 6);
        for row in signals {
            let direction = match row["direction"].as_str().unwrap() {
                "down" => IVec3::NEG_Y,
                "up" => IVec3::Y,
                "north" => IVec3::NEG_Z,
                "south" => IVec3::Z,
                "west" => IVec3::NEG_X,
                "east" => IVec3::X,
                _ => unreachable!(),
            };
            let signal = parse_sensor(row).signal(direction).unwrap();
            assert_eq!(
                (signal.0 as u64, signal.1 as u64),
                (
                    row["weak"].as_u64().unwrap(),
                    row["direct"].as_u64().unwrap()
                ),
                "{row}"
            );
            if direction == IVec3::NEG_Y {
                let block = parse_sensor(row).to_block().unwrap();
                for (kind, boxes) in [
                    (
                        "outline",
                        native_world::collision::block_outline(&block).unwrap(),
                    ),
                    (
                        "collision",
                        native_world::collision::block_shape(&block).unwrap(),
                    ),
                ] {
                    assert_eq!(
                        boxes.len() as u64,
                        row[format!("{kind}_count")].as_u64().unwrap(),
                        "{row}"
                    );
                    assert_eq!(boxes.len(), 1, "{row}");
                    let bounds = [
                        boxes[0].min.x,
                        boxes[0].min.y,
                        boxes[0].min.z,
                        boxes[0].max.x,
                        boxes[0].max.y,
                        boxes[0].max.z,
                    ];
                    for (actual, expected) in bounds.into_iter().zip(row[kind].as_array().unwrap())
                    {
                        assert_eq!(f64::from(actual), expected.as_f64().unwrap(), "{row}");
                    }
                }
            }
        }
        let sensor_filter = data["sensor_filter"].as_array().unwrap();
        assert_eq!(sensor_filter.len(), 61 * 3 * 2 * 17);
        for row in sensor_filter {
            if row["calibrated"].as_bool().unwrap() {
                assert_eq!(parse_sensor(row).back_face(), Some(IVec3::Z));
                assert!(row["back_is_south"].as_bool().unwrap(), "{row}");
            }
            assert_eq!(
                parse_sensor(row).can_receive(
                    row["event"].as_str().unwrap(),
                    row["same"].as_bool().unwrap(),
                    Some(row["input"].as_u64().unwrap() as u8)
                ),
                Some(row["accepted"].as_bool().unwrap()),
                "{row}"
            );
        }
        let ticks = data["sensor_ticks"].as_array().unwrap();
        assert_eq!(ticks.len(), 2 * 3 * 16 * 2);
        crate::loaded::tests::compare_saved_sensors_with_java(ticks);
        for row in ticks {
            let before = parse_sensor(row);
            let change = before.scheduled_tick();
            let after = change.as_ref().map_or(before, |v| v.state);
            let phase = match after.phase {
                crate::sculk_sensor::Phase::Inactive => "inactive",
                crate::sculk_sensor::Phase::Active => "active",
                crate::sculk_sensor::Phase::Cooldown => "cooldown",
            };
            assert_eq!(phase, row["after_phase"].as_str().unwrap(), "{row}");
            assert_eq!(
                u64::from(after.power),
                row["after_power"].as_u64().unwrap(),
                "{row}"
            );
            assert_eq!(
                u64::from(change.as_ref().and_then(|v| v.tick_after).unwrap_or(0)),
                row["delay"].as_u64().unwrap(),
                "{row}"
            );
            assert_eq!(
                u64::from(change.as_ref().is_some_and(|v| v.notify_neighbors)) * 2,
                row["notifications"].as_u64().unwrap(),
                "{row}"
            );
            assert_eq!(
                change.as_ref().is_some_and(|v| v.play_stop),
                row["sounds"].as_u64().unwrap() > 0,
                "{row}"
            );
        }
        let arrivals = data["sensor_arrivals"].as_array().unwrap();
        assert_eq!(arrivals.len(), 2 * 3 * 2 * 4 * 5);
        for row in arrivals {
            let before = parse_sensor(row);
            let change = before.arrive(
                row["event"].as_str().unwrap(),
                row["distance"].as_f64().unwrap() as f32,
            );
            let after = change.as_ref().map_or(before, |v| v.state);
            let phase = match after.phase {
                crate::sculk_sensor::Phase::Inactive => "inactive",
                crate::sculk_sensor::Phase::Active => "active",
                crate::sculk_sensor::Phase::Cooldown => "cooldown",
            };
            assert_eq!(phase, row["after_phase"].as_str().unwrap(), "{row}");
            assert_eq!(
                u64::from(after.power),
                row["after_power"].as_u64().unwrap(),
                "{row}"
            );
            assert_eq!(
                u64::from(change.as_ref().and_then(|v| v.tick_after).unwrap_or(0)),
                row["delay"].as_u64().unwrap(),
                "{row}"
            );
            assert_eq!(
                u64::from(change.as_ref().is_some_and(|v| v.notify_neighbors)) * 2,
                row["notifications"].as_u64().unwrap(),
                "{row}"
            );
            assert_eq!(
                change.as_ref().is_some_and(|v| v.play_click),
                row["sounds"].as_u64().unwrap() > 0,
                "{row}"
            );
            assert_eq!(
                change.as_ref().is_some_and(|v| v.emit_clicking_event),
                row["events"].as_u64().unwrap() > 0,
                "{row}"
            );
            assert_eq!(
                u64::from(change.as_ref().and_then(|v| v.last_frequency).unwrap_or(0)),
                row["frequency"].as_u64().unwrap(),
                "{row}"
            );
        }
    }
}
