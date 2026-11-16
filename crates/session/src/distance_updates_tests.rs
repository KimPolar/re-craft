use super::*;
use std::collections::BTreeMap;

use native_persistence::chunk_tickets::TicketKind;
use native_world::loaded::ChunkPos;

use crate::{
    loading_chunks::unloaded_level,
    simulation_player::{PlayerLoadingTicketChange, PlayerSection},
    simulation_tickets::SimulationTickets,
};

fn key(pos: ChunkPos) -> u64 {
    u64::from(pos.x as u32) | (u64::from(pos.z as u32) << 32)
}

fn position(key: u64) -> ChunkPos {
    ChunkPos {
        x: key as u32 as i32,
        z: (key >> 32) as u32 as i32,
    }
}

fn text(pos: ChunkPos) -> String {
    format!("{},{}", pos.x, pos.z)
}

#[derive(Default)]
struct Driver {
    identity: DistanceDriverIdentity,
    holders: BTreeMap<ChunkPos, (HolderIdentity, u8)>,
    hashes: BTreeMap<ChunkPos, u32>,
    events: Vec<String>,
    registrations: Vec<PlayerLoadingRelease>,
    fail_futures: bool,
}

impl DistanceUpdateDriver for Driver {
    fn identity(&self) -> &DistanceDriverIdentity {
        &self.identity
    }

    fn schedule(&mut self, change: LoadingLevelChange) -> Result<Option<HolderIdentity>> {
        let old = self
            .holders
            .get(&change.position)
            .map_or(unloaded_level(), |entry| entry.1);
        if old == change.level {
            return Ok(None);
        }
        self.events.push(format!(
            "S:{}:{old}:{}",
            text(change.position),
            change.level
        ));
        if let Some((identity, level)) = self.holders.get_mut(&change.position) {
            *level = change.level;
            return Ok(Some(*identity));
        }
        if change.level >= unloaded_level() {
            return Ok(None);
        }
        let id = key(change.position);
        let hash = self
            .hashes
            .get(&change.position)
            .copied()
            .unwrap_or((id ^ (id >> 32)) as u32);
        let identity = HolderIdentity {
            id,
            identity_hash: hash,
        };
        self.holders
            .insert(change.position, (identity, change.level));
        Ok(Some(identity))
    }

    fn update_highest_allowed_status(&mut self, holder: HolderIdentity) -> Result<()> {
        self.events.push(format!("H:{}", text(position(holder.id))));
        Ok(())
    }

    fn update_futures(&mut self, holder: HolderIdentity) -> Result<()> {
        self.events.push(format!("F:{}", text(position(holder.id))));
        ensure!(!self.fail_futures, "simulated holder failure");
        Ok(())
    }

    fn register_player_loading_release(&mut self, token: PlayerLoadingRelease) -> Result<()> {
        self.registrations.push(token);
        Ok(())
    }
}

fn pump_player(player: &mut PlayerSimulation, changes: &mut Vec<PlayerLoadingTicketChange>) {
    for _ in 0..100 {
        while player.step_player_loading_dispatcher() {}
        if !player.step_player_loading_main(|change| changes.push(change)) {
            break;
        }
    }
    while player.step_player_loading_dispatcher() {}
}

#[test]
fn updates_holders_before_registering_player_releases() -> Result<()> {
    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
    player.set_loading_view_distance(2);
    player.observe_player(PlayerSection { x: 0, y: 0, z: 0 }, false, true);
    player.run_updates();
    let mut changes = Vec::new();
    pump_player(&mut player, &mut changes);
    assert_eq!(changes.len(), 4);
    let mut updates = DistanceUpdates::default();
    let mut driver = Driver::default();
    assert!(updates.run(&mut player, &mut driver)?);
    assert!(driver.registrations.is_empty());
    let first_highest = driver
        .events
        .iter()
        .position(|event| event.starts_with("H:"))
        .unwrap();
    let first_future = driver
        .events
        .iter()
        .position(|event| event.starts_with("F:"))
        .unwrap();
    assert!(
        driver.events[..first_highest]
            .iter()
            .all(|event| event.starts_with("S:"))
    );
    assert!(
        driver.events[first_highest..first_future]
            .iter()
            .all(|event| event.starts_with("H:"))
    );
    assert!(
        driver.events[first_future..]
            .iter()
            .all(|event| event.starts_with("F:"))
    );
    let highest: Vec<_> = driver.events[first_highest..first_future]
        .iter()
        .map(|event| &event[2..])
        .collect();
    let futures: Vec<_> = driver.events[first_future..]
        .iter()
        .map(|event| &event[2..])
        .collect();
    assert_eq!(highest, futures);
    assert!(!updates.run(&mut player, &mut driver)?);
    assert_eq!(driver.registrations.len(), 4);
    Ok(())
}

#[test]
fn duplicate_holder_updates_are_coalesced_and_failure_is_terminal() -> Result<()> {
    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
    let mut updates = DistanceUpdates::default();
    let mut driver = Driver::default();
    let identity = HolderIdentity {
        id: key(ChunkPos { x: 5, z: -1 }),
        identity_hash: 42,
    };
    updates.mark_holder_changed(identity)?;
    updates.mark_holder_changed(identity)?;
    assert!(
        updates
            .mark_holder_changed(HolderIdentity {
                identity_hash: 43,
                ..identity
            })
            .is_err()
    );
    assert!(updates.run(&mut player, &mut driver)?);
    assert_eq!(driver.events, ["H:5,-1", "F:5,-1"]);
    assert!(!updates.run(&mut player, &mut driver)?);
    updates.mark_holder_changed(identity)?;
    driver.fail_futures = true;
    assert!(updates.run(&mut player, &mut driver).is_err());
    assert!(updates.run(&mut player, &mut driver).is_err());
    assert!(updates.mark_holder_changed(identity).is_err());
    Ok(())
}

#[test]
fn requires_loading_tracking_without_mutating_the_player() {
    let mut player = PlayerSimulation::new(SimulationTickets::default(), 2);
    let mut updates = DistanceUpdates::default();
    let mut driver = Driver::default();
    assert!(updates.run(&mut player, &mut driver).is_err());
    assert!(driver.events.is_empty());
    assert!(updates.changed.is_empty());
}

#[test]
fn coordinator_rejects_another_session_or_holder_driver() -> Result<()> {
    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
    let mut other_player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
    let mut driver = Driver::default();
    let mut other_driver = Driver::default();
    let mut updates = DistanceUpdates::default();
    assert!(!updates.run(&mut player, &mut driver)?);
    assert!(updates.run(&mut other_player, &mut driver).is_err());
    assert!(updates.run(&mut player, &mut other_driver).is_err());
    assert!(other_driver.events.is_empty());
    assert!(!updates.run(&mut player, &mut driver)?);
    Ok(())
}

struct CoordinatedHolder {
    identity: HolderIdentity,
    level: u8,
    block: crate::ticking_preparation::TickingHolder,
    entity: crate::ticking_preparation::EntityTickingHolder,
}

struct CoordinatedDriver {
    identity: DistanceDriverIdentity,
    holders: BTreeMap<ChunkPos, CoordinatedHolder>,
    futures: BTreeMap<ChunkPos, crate::ticking_preparation::FullFutureId>,
    queue: crate::ticking_preparation::TickingPreparations,
    links: crate::ticking_preparation::EntityLoadingLinks,
    next_block: u64,
    next_entity: u64,
}

impl CoordinatedDriver {
    fn new() -> Result<Self> {
        let queue = crate::ticking_preparation::TickingPreparations::with_entity_tracking();
        let links = crate::ticking_preparation::EntityLoadingLinks::new(&queue)?;
        Ok(Self {
            identity: DistanceDriverIdentity::default(),
            holders: BTreeMap::new(),
            futures: BTreeMap::new(),
            queue,
            links,
            next_block: 0,
            next_entity: 0,
        })
    }

    fn deliver_events(&mut self, player: &mut PlayerSimulation) -> Result<usize> {
        use crate::ticking_preparation::TickingExecutorEvent;
        let mut entities = 0;
        while let Some(event) = self.queue.next_executor_event() {
            match event {
                TickingExecutorEvent::BlockQueued(expected) => {
                    let (actual, consumed) = self
                        .queue
                        .dispatch_next(|preparation| {
                            preparation.dispatch(|_| Ok(()), |_| Ok(()), |_| Ok(()))
                        })?
                        .expect("queued callback");
                    assert_eq!(actual, expected);
                    assert!(consumed);
                }
                TickingExecutorEvent::EntityListener { listener, state } => {
                    self.links.deliver(&self.queue, player, listener, state)?;
                    entities += 1;
                }
            }
        }
        Ok(entities)
    }
}

impl DistanceUpdateDriver for CoordinatedDriver {
    fn identity(&self) -> &DistanceDriverIdentity {
        &self.identity
    }

    fn schedule(&mut self, change: LoadingLevelChange) -> Result<Option<HolderIdentity>> {
        use crate::ticking_preparation::{EntityTickingHolder, FullFutureId, TickingHolder};
        if let Some(holder) = self.holders.get_mut(&change.position) {
            if holder.level == change.level {
                return Ok(None);
            }
            holder.level = change.level;
            holder.block.set_ticket_level(i32::from(change.level));
            holder.entity.set_ticket_level(i32::from(change.level));
            return Ok(Some(holder.identity));
        }
        if change.level >= unloaded_level() {
            return Ok(None);
        }
        let id = key(change.position);
        let identity = HolderIdentity {
            id,
            identity_hash: (id ^ (id >> 32)) as u32,
        };
        self.futures.insert(change.position, FullFutureId(id));
        self.holders.insert(
            change.position,
            CoordinatedHolder {
                identity,
                level: change.level,
                block: TickingHolder::new(change.position, i32::from(change.level), &self.queue),
                entity: EntityTickingHolder::new(
                    change.position,
                    i32::from(change.level),
                    &self.queue,
                ),
            },
        );
        Ok(Some(identity))
    }

    fn update_highest_allowed_status(&mut self, _holder: HolderIdentity) -> Result<()> {
        Ok(())
    }

    fn update_futures(&mut self, identity: HolderIdentity) -> Result<()> {
        use crate::ticking_preparation::{EntityRequestId, FullFuture, TickingRequestId};
        let holder = self
            .holders
            .get_mut(&position(identity.id))
            .expect("scheduled holder");
        let block_id = if holder.block.needs_new_ticking_request() {
            let id = TickingRequestId(self.next_block);
            self.next_block += 1;
            Some(id)
        } else {
            None
        };
        let entity_id = if holder.entity.needs_new_entity_request() {
            let id = EntityRequestId(self.next_entity);
            self.next_entity += 1;
            Some(id)
        } else {
            None
        };
        let futures = &self.futures;
        let full = |position| {
            futures
                .get(&position)
                .copied()
                .map(|id| FullFuture { id, result: None })
        };
        holder.block.update(&mut self.queue, block_id, full)?;
        holder.entity.update(&mut self.queue, entity_id, full)?;
        Ok(())
    }

    fn register_player_loading_release(&mut self, token: PlayerLoadingRelease) -> Result<()> {
        let holder = self
            .holders
            .get(&token.position())
            .expect("player-loading holder");
        self.links.register(&mut self.queue, &holder.entity, token)
    }
}

#[test]
fn distance_update_connects_holders_to_player_loading_future() -> Result<()> {
    use crate::ticking_preparation::{FullFutureResult, entity_ticking_dependencies};
    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
    player.set_loading_view_distance(2);
    player.observe_player(PlayerSection { x: 0, y: 0, z: 0 }, false, true);
    player.run_updates();
    let mut changes = Vec::new();
    pump_player(&mut player, &mut changes);
    assert_eq!(changes.len(), 4);
    let mut updates = DistanceUpdates::default();
    let mut driver = CoordinatedDriver::new()?;
    assert!(updates.run(&mut player, &mut driver)?);
    assert_eq!(driver.links.pending_len(), 0);
    assert!(!updates.run(&mut player, &mut driver)?);
    assert_eq!(driver.links.pending_len(), 4);
    let center = ChunkPos { x: 0, z: 0 };
    let dependencies = entity_ticking_dependencies(center);
    for pos in dependencies.iter().take(24) {
        driver
            .queue
            .complete(driver.futures[pos], FullFutureResult::Full)?;
    }
    assert_eq!(driver.deliver_events(&mut player)?, 0);
    pump_player(&mut player, &mut changes);
    assert_eq!(changes.len(), 4);
    driver
        .queue
        .complete(driver.futures[&dependencies[24]], FullFutureResult::Full)?;
    assert_eq!(driver.deliver_events(&mut player)?, 1);
    assert_eq!(driver.links.pending_len(), 3);
    pump_player(&mut player, &mut changes);
    assert_eq!(changes.len(), 5);
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn distance_update_phases_match_java_26_3() -> Result<()> {
    let oracle = crate::worldgen::postprocessing_java_results(
        "DistanceUpdateOracle",
        include_str!("../tests/java/DistanceUpdateOracle.java"),
        &[],
    );
    let mut driver = Driver::default();
    let mut expected = Vec::new();
    for line in oracle {
        if let Some(identity) = line.strip_prefix("ID|") {
            let (coords, hash) = identity.split_once('|').unwrap();
            let (x, z) = coords.split_once(',').unwrap();
            driver.hashes.insert(
                ChunkPos {
                    x: x.parse()?,
                    z: z.parse()?,
                },
                hash.parse()?,
            );
        } else {
            expected.push(line);
        }
    }
    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
    let mut updates = DistanceUpdates::default();
    let a = ChunkPos { x: 0, z: 0 };
    let b = ChunkPos { x: 3, z: -2 };
    let unloaded = i32::from(unloaded_level());
    let mut actual = Vec::new();
    let run = |name: &str,
               player: &mut PlayerSimulation,
               updates: &mut DistanceUpdates,
               driver: &mut Driver|
     -> Result<String> {
        driver.events.clear();
        let more = updates.run(player, driver)?;
        Ok(format!("TRACE|{name}|{more}|{}", driver.events.join(";")))
    };
    player.add_ticket(a, TicketKind::Unknown, unloaded - 3);
    player.add_ticket(b, TicketKind::Forced, unloaded - 2);
    actual.push(run("add", &mut player, &mut updates, &mut driver)?);
    actual.push(run("settled", &mut player, &mut updates, &mut driver)?);
    player.remove_ticket(a, TicketKind::Unknown, unloaded - 3);
    actual.push(run("remove", &mut player, &mut updates, &mut driver)?);
    player.add_ticket(a, TicketKind::Unknown, unloaded - 2);
    actual.push(run("return", &mut player, &mut updates, &mut driver)?);
    player.remove_ticket(b, TicketKind::Forced, unloaded - 2);
    actual.push(run("remove_other", &mut player, &mut updates, &mut driver)?);
    actual.push(run(
        "settled_again",
        &mut player,
        &mut updates,
        &mut driver,
    )?);
    assert_eq!(actual, expected);
    Ok(())
}
