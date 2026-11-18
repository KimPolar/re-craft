use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use super::*;
use crate::{
    distance_updates::DistanceUpdates,
    simulation_player::{PlayerLoadingTicketChange, PlayerSection},
    simulation_tickets::SimulationTickets,
};

fn pump_player(player: &mut PlayerSimulation, changes: &mut Vec<PlayerLoadingTicketChange>) {
    for _ in 0..100 {
        while player.step_player_loading_dispatcher() {}
        if !player.step_player_loading_main(|change| changes.push(change)) {
            break;
        }
    }
    while player.step_player_loading_dispatcher() {}
}

fn dispatch_events(holders: &mut LoadingHolders, player: &mut PlayerSimulation) -> Result<()> {
    while let Some(event) = holders.peek_executor_event() {
        assert_eq!(holders.next_executor_event(), Some(event));
        match event {
            TickingExecutorEvent::BlockQueued(expected) => {
                let (actual, consumed) = holders
                    .preparations_mut()
                    .dispatch_next(|callback| {
                        callback.dispatch(|_| Ok(()), |_| Ok(()), |_| Ok(()))
                    })?
                    .context("block event has no matching callback")?;
                ensure!(
                    actual == expected && consumed,
                    "block callback order changed"
                );
            }
            TickingExecutorEvent::EntityListener { listener, state } => {
                holders.deliver_entity_event(player, listener, state)?;
            }
        }
    }
    holders.reap_retired();
    Ok(())
}

#[test]
fn player_loading_waits_for_actual_full_completions() -> Result<()> {
    let center = ChunkPos { x: 0, z: 0 };
    let mut player = PlayerSimulation::new(SimulationTickets::with_loading_tracker(), 2);
    player.set_loading_view_distance(2);
    player.observe_player(PlayerSection { x: 0, y: 0, z: 0 }, false, true);
    player.run_updates();
    let mut changes = Vec::new();
    pump_player(&mut player, &mut changes);
    assert_eq!(changes.len(), 4);

    let mut updates = DistanceUpdates::default();
    let mut holders = LoadingHolders::new()?;
    assert!(updates.run(&mut player, &mut holders)?);
    assert!(!updates.run(&mut player, &mut holders)?);
    assert_eq!(holders.block_state(center)?, TickingFutureState::Waiting);
    assert_eq!(holders.entity_state(center)?, TickingFutureState::Waiting);

    let mut requested = BTreeMap::new();
    while let Some(request) = holders.next_generation_request() {
        let position = request.position();
        assert!(requested.insert(position, request).is_none());
    }
    let neighbors = entity_ticking_dependencies(center);
    for &position in neighbors.iter().take(24) {
        holders.complete_full_generation(
            requested
                .remove(&position)
                .context("entity neighbor was not requested")?,
        )?;
    }
    assert_eq!(holders.entity_state(center)?, TickingFutureState::Waiting);
    pump_player(&mut player, &mut changes);
    assert_eq!(changes.len(), 4);
    let last = requested
        .remove(&neighbors[24])
        .context("last entity neighbor was not requested")?;
    holders.defer_generation_request(last)?;
    let retry = holders
        .next_generation_request()
        .context("deferred generation request was lost")?;
    assert_eq!(retry.position(), neighbors[24]);
    holders.complete_full_generation(retry)?;
    assert_eq!(holders.entity_state(center)?, TickingFutureState::Available);
    // FULL completion only queues the block callback; it does not execute it.
    assert_eq!(holders.block_state(center)?, TickingFutureState::Waiting);
    dispatch_events(&mut holders, &mut player)?;
    assert_eq!(holders.block_state(center)?, TickingFutureState::Available);
    pump_player(&mut player, &mut changes);
    assert!(changes.len() > 4);
    Ok(())
}

#[test]
fn unloaded_holder_waits_for_old_work_and_rejects_stale_completion() -> Result<()> {
    let center = ChunkPos { x: 0, z: 0 };
    let mut holders = LoadingHolders::new()?;
    let mut identities = BTreeMap::new();
    for position in ticking_dependencies(center) {
        let level = if position == center { 32 } else { 33 };
        let identity = holders
            .schedule(LoadingLevelChange {
                position,
                previous: unloaded_level(),
                level,
            })?
            .context("holder was not created")?;
        identities.insert(position, identity);
    }
    for &identity in identities.values() {
        holders.update_highest_allowed_status(identity)?;
    }
    let original = identities[&center];
    holders.update_futures(original)?;
    let request = loop {
        let request = holders
            .next_generation_request()
            .context("center generation request missing")?;
        if request.position() == center {
            break request;
        }
    };
    let stale = FullGenerationRequest {
        position: request.position,
        owner: request.owner.clone(),
        task: request.task,
    };
    holders.schedule(LoadingLevelChange {
        position: center,
        previous: 32,
        level: unloaded_level(),
    })?;
    holders.update_highest_allowed_status(original)?;
    holders.update_futures(original)?;
    assert!(holders.retire_unloaded_holder(center).is_err());
    holders.discard_unneeded_generation_request(request)?;
    assert!(holders.retire_unloaded_holder(center)?);
    assert_eq!(holders.preparations_mut().tracked_full_future_count(), 0);
    assert_eq!(
        holders.block_state(center)?,
        TickingFutureState::Unavailable
    );

    let replacement = holders
        .schedule(LoadingLevelChange {
            position: center,
            previous: unloaded_level(),
            level: 32,
        })?
        .context("replacement holder was not created")?;
    assert_ne!(original.id, replacement.id);
    holders.update_highest_allowed_status(replacement)?;
    holders.update_futures(replacement)?;
    let current = loop {
        let request = holders
            .next_generation_request()
            .context("replacement generation request missing")?;
        if request.position() == center {
            break request;
        }
    };
    assert!(holders.complete_full_generation(stale).is_err());
    holders.complete_full_generation(current)?;
    assert_eq!(holders.block_state(center)?, TickingFutureState::Waiting);
    Ok(())
}
