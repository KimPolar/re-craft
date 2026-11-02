//! Java 26.3 game-event frequencies and vibration candidate selection.
//!
//! This module does not yet discover listeners, test occlusion, or activate
//! sculk sensors. Callers must only submit events that passed those checks.

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
        let mut resources = native_resources::packs::ResourceStack::open(&game).unwrap();
        let tags =
            native_resources::game_event_tags::GameEventTags::load(&mut resources, &registry)
                .unwrap();
        assert!(tags.vibrates("minecraft:splash"));
        assert!(tags.ignored_while_sneaking("minecraft:step"));
        assert!(!tags.ignored_while_sneaking("minecraft:splash"));
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
    }
}
