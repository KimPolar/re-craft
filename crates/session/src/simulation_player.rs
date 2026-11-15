//! The local player's Java 26.3 simulation-ticket lifecycle.
//!
//! This only drives the ticket graph. Chunk loading, ticking-future completion,
//! scheduled tick unpacking, and timed-ticket expiration remain caller duties.

use std::collections::BTreeSet;

use native_core::random::LegacyRandom;
use native_persistence::chunk_tickets::TicketKind;
use native_world::loaded::ChunkPos;

use crate::simulation_chunks::SimulationChunks;
use crate::simulation_tickets::SimulationTickets;

/// ChunkMap compares packed section positions, including the vertical section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerSection {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl PlayerSection {
    fn packed(self) -> u64 {
        ((self.x as u64 & 0x3f_ffff) << 42)
            | ((self.z as u64 & 0x3f_ffff) << 20)
            | (self.y as u64 & 0x0f_ffff)
    }

    fn chunk(self) -> ChunkPos {
        ChunkPos {
            x: self.x,
            z: self.z,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TrackedPlayer {
    section: PlayerSection,
    ignored: bool,
}

/// TicketStorage and the player registration share one simulation graph.
#[derive(Clone, Debug)]
pub struct PlayerSimulation {
    tickets: SimulationTickets,
    // The natural-spawn tracker has its own player-centered radius and hash
    // iteration order; forced simulation tickets do not contribute to it.
    natural_spawn: SimulationChunks,
    distance: u32,
    player: Option<TrackedPlayer>,
}

impl PlayerSimulation {
    /// `distance` is the server's initial distance, set before saved inactive
    /// tickets are activated. No existing ticket is rewritten at this point.
    pub fn new(tickets: SimulationTickets, distance: u32) -> Self {
        Self {
            tickets,
            natural_spawn: SimulationChunks::default(),
            distance,
            player: None,
        }
    }

    pub fn tickets(&self) -> &SimulationTickets {
        &self.tickets
    }

    /// Other game systems can add portal, forced, or entity tickets here.
    pub fn add_ticket(&mut self, position: ChunkPos, kind: TicketKind, level: i32) -> bool {
        self.tickets.add_ticket(position, kind, level)
    }

    pub fn remove_ticket(&mut self, position: ChunkPos, kind: TicketKind, level: i32) -> bool {
        self.tickets.remove_ticket(position, kind, level)
    }

    pub fn tracker(&self) -> &SimulationChunks {
        self.tickets.tracker()
    }

    /// Java's separate eight-chunk player-distance map, before holder and
    /// spectator-distance filtering. Its order affects the spawning shuffle.
    /// No order is exposed while this tracker is still propagating.
    pub fn natural_spawn_candidates(&self) -> Option<impl Iterator<Item = ChunkPos> + '_> {
        if self.natural_spawn.has_pending_updates() {
            return None;
        }
        Some(self.natural_spawn.block_ticking_order())
    }

    /// Apply ChunkMap's holder and horizontal player-distance checks. A caller
    /// must supply actual ticking-future readiness, not merely loaded chunks.
    pub fn collect_spawning_chunks(
        &self,
        player_x: f64,
        player_z: f64,
        spectator: bool,
        mut is_ticking: impl FnMut(ChunkPos) -> bool,
    ) -> Option<Vec<ChunkPos>> {
        Some(
            self.natural_spawn_candidates()?
                .filter(|&pos| {
                    if !is_ticking(pos) || spectator {
                        return false;
                    }
                    let center_x = f64::from(pos.x.wrapping_shl(4).wrapping_add(8));
                    let center_z = f64::from(pos.z.wrapping_shl(4).wrapping_add(8));
                    let dx = center_x - player_x;
                    let dz = center_z - player_z;
                    dx * dx + dz * dz < 16_384.0
                })
                .collect(),
        )
    }

    /// Shuffle a collected candidate list exactly as Java's Util.shuffle.
    /// The list comes from a single player's tracker and is at most 289 long.
    pub fn shuffle_spawning_chunks(chunks: &mut [ChunkPos], random: &mut LegacyRandom) {
        for end in (1..chunks.len()).rev() {
            let bound = i32::try_from(end + 1).expect("spawning chunk count fits in i32");
            let selected = random
                .next_int_bound(bound)
                .expect("positive shuffle bound") as usize;
            chunks.swap(end, selected);
        }
    }

    pub fn has_pending_updates(&self) -> bool {
        self.natural_spawn.has_pending_updates() || self.tracker().has_pending_updates()
    }

    pub fn distance(&self) -> u32 {
        self.distance
    }

    /// Potential FULL dependencies of block-ticking candidates. Java's
    /// PLAYER_SIMULATION ticket does not load chunks: the player-loading
    /// window and other loading tickets must also cover the candidate.
    /// Actual holders, throttling, and ticking futures remain separate.
    pub fn full_tick_dependencies(&self, view_distance: u32) -> Option<BTreeSet<ChunkPos>> {
        if self.tracker().has_pending_updates() {
            return None;
        }
        let player_center = self
            .player
            .filter(|player| !player.ignored)
            .map(|player| player.section.chunk());
        let player_radius = i64::from(view_distance.clamp(2, 32)) + 1;
        let mut other_loading = BTreeSet::new();
        for ticket in self.tickets.tickets().filter(|ticket| ticket.kind.loads()) {
            if ticket.level > 32 {
                continue;
            }
            let radius = 32 - ticket.level.clamp(0, 32);
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    other_loading.insert(ChunkPos {
                        x: ticket.position.x.wrapping_add(dx),
                        z: ticket.position.z.wrapping_add(dz),
                    });
                }
            }
        }
        let mut result = BTreeSet::new();
        for center in self.tracker().block_ticking_order() {
            let player_loads = player_center.is_some_and(|player| {
                (i64::from(center.x) - i64::from(player.x)).abs() <= player_radius
                    && (i64::from(center.z) - i64::from(player.z)).abs() <= player_radius
            });
            if !player_loads && !other_loading.contains(&center) {
                continue;
            }
            for dx in -1_i32..=1 {
                for dz in -1_i32..=1 {
                    let position = ChunkPos {
                        x: center.x.wrapping_add(dx),
                        z: center.z.wrapping_add(dz),
                    };
                    if position.x.checked_mul(16).is_some() && position.z.checked_mul(16).is_some()
                    {
                        result.insert(position);
                    }
                }
            }
        }
        Some(result)
    }

    pub fn player_loading_center(&self) -> Option<ChunkPos> {
        self.player
            .filter(|player| !player.ignored)
            .map(|player| player.section.chunk())
    }

    /// Mirror `DistanceManager.updateSimulationDistance`; changing the raw
    /// distance still replaces tickets when both values clamp to level zero.
    pub fn set_distance(&mut self, distance: u32) {
        if distance != self.distance {
            self.distance = distance;
            self.tickets.replace_level(
                TicketKind::PlayerSimulation,
                SimulationChunks::player_ticket_level(distance),
            );
        }
    }

    /// Apply `ChunkMap.updatePlayerStatus` or `ChunkMap.move` for the one
    /// local player. Vertical movement can remove and re-add the same ticket.
    pub fn observe_player(
        &mut self,
        section: PlayerSection,
        spectator: bool,
        spectators_generate_chunks: bool,
    ) {
        let ignored = spectator && !spectators_generate_chunks;
        if let Some(previous) = self.player {
            if previous.section.packed() == section.packed() && previous.ignored == ignored {
                return;
            }
            if !previous.ignored {
                self.tickets.remove_ticket(
                    previous.section.chunk(),
                    TicketKind::PlayerSimulation,
                    SimulationChunks::player_ticket_level(self.distance),
                );
                self.natural_spawn
                    .remove_ticket(previous.section.chunk(), 0, 24);
            }
        }
        if !ignored {
            self.tickets.add_ticket(
                section.chunk(),
                TicketKind::PlayerSimulation,
                SimulationChunks::player_ticket_level(self.distance),
            );
            // Offsetting the distance by 24 gives levels 24..=32 over the
            // same radius 0..=8 as FixedPlayerDistanceChunkTracker.
            self.natural_spawn.add_ticket(section.chunk(), 0, 24);
        }
        self.player = Some(TrackedPlayer { section, ignored });
    }

    pub fn remove_player(&mut self) {
        if let Some(previous) = self.player.take()
            && !previous.ignored
        {
            self.tickets.remove_ticket(
                previous.section.chunk(),
                TicketKind::PlayerSimulation,
                SimulationChunks::player_ticket_level(self.distance),
            );
            self.natural_spawn
                .remove_ticket(previous.section.chunk(), 0, 24);
        }
    }

    /// Process graph changes only when the caller reaches a server update.
    pub fn run_updates(&mut self) {
        self.natural_spawn.run_updates();
        self.tickets.run_updates();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_changes_and_spectator_rule_update_tickets_without_render_distance() {
        let mut live = PlayerSimulation::new(SimulationTickets::default(), 5);
        let first = PlayerSection { x: -2, y: 4, z: 3 };
        live.observe_player(first, false, false);
        live.run_updates();
        assert_eq!(live.tracker().level(first.chunk()), 26);
        let vertical = PlayerSection { y: 5, ..first };
        live.observe_player(vertical, false, false);
        live.run_updates();
        assert_eq!(live.tickets().tickets().count(), 1);
        assert_eq!(live.tracker().level(first.chunk()), 26);
        live.observe_player(vertical, true, false);
        live.run_updates();
        assert_eq!(live.tickets().tickets().count(), 0);
        live.observe_player(vertical, true, true);
        live.set_distance(12);
        live.run_updates();
        assert_eq!(live.tracker().level(first.chunk()), 19);
        live.remove_player();
        live.run_updates();
        assert_eq!(live.tickets().tickets().count(), 0);
    }

    #[test]
    fn dependencies_require_both_simulation_and_loading_and_include_block_tick_halo() {
        let mut live = PlayerSimulation::new(SimulationTickets::default(), 12);
        live.observe_player(PlayerSection { x: 0, y: 0, z: 0 }, false, true);
        assert!(live.full_tick_dependencies(2).is_none());
        live.run_updates();
        let small = live.full_tick_dependencies(2).unwrap();
        assert!(small.contains(&ChunkPos { x: 4, z: 0 }));
        assert!(!small.contains(&ChunkPos { x: 5, z: 0 }));
        let normal = live.full_tick_dependencies(12).unwrap();
        assert!(normal.contains(&ChunkPos { x: 14, z: 0 }));
        assert!(!normal.contains(&ChunkPos { x: 15, z: 0 }));

        let forced = ChunkPos { x: 100, z: 0 };
        live.add_ticket(forced, TicketKind::Forced, 31);
        live.run_updates();
        let with_forced = live.full_tick_dependencies(2).unwrap();
        assert!(with_forced.contains(&ChunkPos { x: 102, z: 0 }));
        assert!(!with_forced.contains(&ChunkPos { x: 103, z: 0 }));
        live.observe_player(PlayerSection { x: 0, y: 0, z: 0 }, true, false);
        live.run_updates();
        let ignored = live.full_tick_dependencies(2).unwrap();
        assert!(!ignored.contains(&ChunkPos { x: 0, z: 0 }));
        assert!(ignored.contains(&forced));

        let mut tickets = SimulationTickets::default();
        tickets.add_ticket(ChunkPos { x: 0, z: 0 }, TicketKind::PlayerSimulation, 31);
        let mut inactive = PlayerSimulation::new(tickets, 12);
        inactive.run_updates();
        assert!(inactive.full_tick_dependencies(12).unwrap().is_empty());
    }
}
