//! Loading-ticket levels used to schedule chunk holders in Java 26.3.
//!
//! These levels do not establish that a holder's futures are ready. The caller
//! must also deliver player-loading tickets, holder updates and task results
//! in their real order before using this graph to activate ticking chunks.

use native_world::loaded::ChunkPos;

use crate::{simulation_chunks::SimulationChunks, worldgen::schedule::GenerationStage};

/// One call to the loading graph's holder scheduling operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadingLevelChange {
    pub position: ChunkPos,
    pub previous: u8,
    pub level: u8,
}

/// The first level outside Java's loaded chunk range.
pub fn unloaded_level() -> u8 {
    let radius = GenerationStage::Full.accumulated().len().saturating_sub(1);
    u8::try_from(34 + radius).expect("generation radius fits in a chunk level")
}

/// Propagate the loading tickets already delivered to the shared ticket store.
/// Player-ticket throttling and chunk-holder lifecycle are separate duties.
#[derive(Clone, Debug)]
pub struct LoadingChunks {
    graph: SimulationChunks,
}

impl Default for LoadingChunks {
    fn default() -> Self {
        Self {
            graph: SimulationChunks::with_max_level(unloaded_level()),
        }
    }
}

impl LoadingChunks {
    pub(crate) fn add_ticket(&mut self, position: ChunkPos, kind: u64, level: i32) -> bool {
        self.graph.add_ticket(position, kind, level)
    }

    pub(crate) fn remove_ticket(&mut self, position: ChunkPos, kind: u64, level: i32) -> bool {
        self.graph.remove_ticket(position, kind, level)
    }

    pub(crate) fn remove_expired_tickets(&mut self, position: ChunkPos, tickets: &[(u64, i32)]) {
        self.graph.remove_expired_tickets(position, tickets);
    }

    pub fn level(&self, position: ChunkPos) -> u8 {
        self.graph.level(position)
    }

    pub fn has_pending_updates(&self) -> bool {
        self.graph.has_pending_updates()
    }

    /// Process the same ordered graph work as `runDistanceUpdates`. The
    /// observer receives each level transition, including an intermediate
    /// unloaded level during increases. It does not represent future updates.
    pub fn run_updates_budget(
        &mut self,
        budget: usize,
        mut on_change: impl FnMut(LoadingLevelChange),
    ) -> usize {
        self.graph
            .run_updates_budget_observed(budget, |position, previous, level| {
                on_change(LoadingLevelChange {
                    position,
                    previous,
                    level,
                });
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation_tickets::{ChunkSaveReadiness, SimulationTickets};
    use native_persistence::chunk_tickets::TicketKind;

    #[test]
    fn loading_ticket_work_resumes_without_reordering() {
        let mut tickets = SimulationTickets::with_loading_tracker();
        tickets.add_ticket(ChunkPos { x: 0, z: 0 }, TicketKind::PlayerLoading, 31);
        tickets.add_ticket(ChunkPos { x: 2, z: -2 }, TicketKind::Forced, 30);
        tickets.add_ticket(ChunkPos { x: 4, z: 1 }, TicketKind::PlayerSimulation, 19);
        let mut budgeted = tickets.clone();
        let mut whole_events = Vec::new();
        let mut budget_events = Vec::new();
        tickets
            .run_loading_updates(usize::MAX, |event| whole_events.push(event))
            .unwrap();
        while budgeted.loading_tracker().unwrap().has_pending_updates() {
            budgeted
                .run_loading_updates(3, |event| budget_events.push(event))
                .unwrap();
        }
        assert_eq!(whole_events, budget_events);
        assert_eq!(
            tickets
                .loading_tracker()
                .unwrap()
                .level(ChunkPos { x: 0, z: 0 }),
            31
        );
        assert_eq!(
            tickets
                .loading_tracker()
                .unwrap()
                .level(ChunkPos { x: 2, z: -2 }),
            30
        );
        assert_eq!(
            tickets
                .loading_tracker()
                .unwrap()
                .level(ChunkPos { x: 100, z: 100 }),
            unloaded_level()
        );
        assert!(tickets.tracker().has_pending_updates());
    }

    #[test]
    fn expired_loading_tickets_reveal_weaker_tickets() {
        let pos = ChunkPos { x: -2, z: 3 };
        let mut tickets = SimulationTickets::with_loading_tracker();
        tickets.add_ticket(pos, TicketKind::Unknown, 31);
        tickets.add_ticket(pos, TicketKind::PlayerLoading, 33);
        tickets.add_ticket(pos, TicketKind::PlayerSimulation, 19);
        tickets.run_loading_updates(usize::MAX, |_| {}).unwrap();
        assert_eq!(tickets.loading_tracker().unwrap().level(pos), 31);
        assert_eq!(tickets.purge_stale(|_| ChunkSaveReadiness::Pending), 0);
        assert_eq!(tickets.purge_stale(|_| ChunkSaveReadiness::Pending), 1);
        let mut events = Vec::new();
        tickets
            .run_loading_updates(usize::MAX, |event| events.push(event))
            .unwrap();
        assert_eq!(tickets.loading_tracker().unwrap().level(pos), 33);
        assert!(events.iter().any(|event| {
            event.position == pos && event.previous == 31 && event.level == unloaded_level()
        }));
        tickets.run_updates();
        assert_eq!(tickets.tracker().level(pos), 19);
    }

    #[test]
    fn loading_tracking_is_opt_in_before_ticket_activation() {
        let mut tickets = SimulationTickets::default();
        tickets.add_ticket(ChunkPos { x: 0, z: 0 }, TicketKind::Forced, 31);
        assert!(tickets.loading_tracker().is_none());
        assert_eq!(tickets.run_loading_updates(1, |_| {}), None);
    }

    #[test]
    fn loading_level_alone_does_not_complete_the_ticking_holder() -> anyhow::Result<()> {
        use crate::ticking_preparation::{
            FullFuture, FullFutureId, FullFutureResult, TickingFutureState, TickingHolder,
            TickingHolderTransition, TickingPreparations, TickingRequestId, ticking_dependencies,
        };

        let center = ChunkPos { x: 0, z: 0 };
        let mut tickets = SimulationTickets::with_loading_tracker();
        tickets.add_ticket(center, TicketKind::PlayerLoading, 31);
        tickets.run_loading_updates(usize::MAX, |_| {}).unwrap();
        let level = tickets.loading_tracker().unwrap().level(center);
        let mut queue = TickingPreparations::default();
        let mut holder = TickingHolder::new(center, i32::from(level), &queue);
        let dependencies = ticking_dependencies(center);
        let future_id =
            |pos: ChunkPos| FullFutureId(u64::from(pos.x as u32) | (u64::from(pos.z as u32) << 32));
        assert_eq!(
            holder.update(&mut queue, Some(TickingRequestId(1)), |pos| Some(
                FullFuture {
                    id: future_id(pos),
                    result: None,
                }
            ))?,
            TickingHolderTransition::Promoted(TickingRequestId(1))
        );
        for position in dependencies.iter().take(8) {
            queue.complete(future_id(*position), FullFutureResult::Full)?;
        }
        assert_eq!(holder.state(&queue)?, TickingFutureState::Waiting);
        assert_eq!(queue.queued_len(), 0);
        queue.complete(future_id(dependencies[8]), FullFutureResult::Full)?;
        assert_eq!(holder.state(&queue)?, TickingFutureState::Waiting);
        assert_eq!(queue.queued_len(), 1);
        let events = std::cell::RefCell::new(Vec::new());
        queue.dispatch_next(|future| {
            future.dispatch(
                |_| {
                    events.borrow_mut().push("postprocess");
                    Ok(())
                },
                |_| {
                    events.borrow_mut().push("unpack");
                    Ok(())
                },
                |_| {
                    events.borrow_mut().push("after-start");
                    Ok(())
                },
            )
        })?;
        assert_eq!(*events.borrow(), ["postprocess", "unpack", "after-start"]);
        assert_eq!(holder.state(&queue)?, TickingFutureState::Available);

        tickets.remove_ticket(center, TicketKind::PlayerLoading, 31);
        tickets.run_loading_updates(usize::MAX, |_| {}).unwrap();
        holder.set_ticket_level(i32::from(tickets.loading_tracker().unwrap().level(center)));
        assert!(matches!(
            holder.update(&mut queue, None, |_| None)?,
            TickingHolderTransition::Demoted { .. }
        ));
        assert_eq!(holder.state(&queue)?, TickingFutureState::Unavailable);
        Ok(())
    }
}
