use std::collections::BTreeMap;

use anyhow::{Context, Result};
use native_world::loaded::ChunkPos;

use super::*;
use crate::ticking_preparation::{
    TickingFutureState, TickingPreparations, TickingRequestId, ticking_dependencies,
};

fn state(future: FullFuture) -> &'static str {
    match future.result {
        None => "waiting",
        Some(FullFutureResult::Full) => "full",
        Some(FullFutureResult::Unavailable) => "unavailable",
        Some(FullFutureResult::Exceptional) => "exception",
    }
}

fn completed(
    expected: FullFutureId,
    event: Option<(FullFutureId, FullFutureResult)>,
) -> Result<FullFuture> {
    let (id, result) = event.context("expected a FULL completion")?;
    anyhow::ensure!(id == expected, "the wrong FULL future completed");
    Ok(FullFuture {
        id,
        result: Some(result),
    })
}

fn lifecycle_trace() -> Result<Vec<String>> {
    let mut ids = FullFutureIds::default();
    let mut holder = FullGenerationHolder::default();
    let owner = holder.owner();
    let initial = holder.request_full(&mut ids)?;
    assert!(holder.update_highest_allowed_status(32).is_none());
    let first = holder.request_full(&mut ids)?;
    let again = holder.request_full(&mut ids)?;
    // A ticket change alone does not apply the highest-status pass.
    let before_demotion = holder.request_full(&mut ids)?;
    let first_cancelled = completed(first.id, holder.update_highest_allowed_status(34))?;
    let unloaded = holder.request_full(&mut ids)?;
    let before_promotion = holder.request_full(&mut ids)?;
    assert!(holder.update_highest_allowed_status(33).is_none());
    let second = holder.request_full(&mut ids)?;
    let second_before_completion = state(second);
    let second_completed = completed(second.id, holder.complete_full(&owner, &mut ids)?)?;
    let current = holder.request_full(&mut ids)?;
    assert!(holder.update_highest_allowed_status(34).is_none());
    let hidden = holder.request_full(&mut ids)?;
    assert!(holder.update_highest_allowed_status(31).is_none());
    let restored = holder.request_full(&mut ids)?;
    let duplicate_rejected = holder.complete_full(&owner, &mut ids).is_err();

    let mut late = FullGenerationHolder::default();
    let late_owner = late.owner();
    late.update_highest_allowed_status(32);
    let late_old = late.request_full(&mut ids)?;
    let late_cancelled = completed(late_old.id, late.update_highest_allowed_status(34))?;
    assert!(late.complete_full(&late_owner, &mut ids)?.is_none());
    let late_hidden = late.request_full(&mut ids)?;
    late.update_highest_allowed_status(33);
    let late_restored = late.request_full(&mut ids)?;

    let mut replaced = FullGenerationHolder::default();
    let replaced_owner = replaced.owner();
    replaced.update_highest_allowed_status(32);
    let replaced_old = replaced.request_full(&mut ids)?;
    let replaced_cancelled =
        completed(replaced_old.id, replaced.update_highest_allowed_status(34))?;
    replaced.update_highest_allowed_status(33);
    let replaced_new = replaced.request_full(&mut ids)?;
    let replaced_done = completed(
        replaced_new.id,
        replaced.complete_full(&replaced_owner, &mut ids)?,
    )?;

    let mut extreme = FullGenerationHolder::default();
    extreme.update_highest_allowed_status(i32::MIN);
    let extreme = extreme.request_full(&mut ids)?;
    Ok(vec![
        format!("initial|{}", state(initial)),
        format!(
            "pending-reused|{}|{}",
            first.id == again.id,
            first.id == before_demotion.id
        ),
        format!(
            "demoted|{}|{}|{}",
            state(first_cancelled),
            state(unloaded),
            initial.id == unloaded.id
        ),
        format!("before-promotion|{}", before_promotion.id == unloaded.id),
        format!(
            "new-future|{}|{}",
            second.id != first.id,
            second_before_completion
        ),
        format!(
            "completed-reused|{}|{}|{}",
            second_completed.id == current.id,
            hidden.id == unloaded.id,
            restored.id == second.id
        ),
        format!("duplicate-rejected|{duplicate_rejected}"),
        format!(
            "late-unloaded|{}|{}|{}",
            state(late_cancelled),
            late_hidden.id == unloaded.id,
            state(late_restored)
        ),
        format!(
            "late-replacement|{}|{}|{}",
            state(replaced_cancelled),
            replaced_old.id != replaced_new.id,
            state(replaced_done)
        ),
        format!("overflow-level|{}", state(extreme)),
    ])
}

#[test]
fn cancelled_full_dependency_does_not_activate_an_old_ticking_request() -> Result<()> {
    let center = ChunkPos { x: 0, z: 0 };
    let positions = ticking_dependencies(center);
    let mut ids = FullFutureIds::default();
    let mut holders = BTreeMap::new();
    let mut futures = BTreeMap::new();
    for position in positions {
        let mut holder = FullGenerationHolder::default();
        holder.update_highest_allowed_status(33);
        futures.insert(position, holder.request_full(&mut ids)?);
        holders.insert(position, holder);
    }
    let mut queue = TickingPreparations::default();
    let first = TickingRequestId(1);
    queue.start(first, center, |position| futures.get(&position).copied())?;
    for position in positions.iter().take(8) {
        let holder = holders.get_mut(position).unwrap();
        let owner = holder.owner();
        let (id, result) = holder
            .complete_full(&owner, &mut ids)?
            .context("pending generation future")?;
        queue.complete(id, result)?;
    }
    assert_eq!(queue.queued_len(), 0);

    let last = holders.get_mut(&positions[8]).unwrap();
    let old_owner = last.owner();
    let (id, result) = last
        .update_highest_allowed_status(34)
        .context("pending FULL cancellation")?;
    queue.complete(id, result)?;
    queue.dispatch_next(|callback| {
        callback.dispatch(
            |_| anyhow::bail!("cancelled request postprocessed"),
            |_| anyhow::bail!("cancelled request unpacked"),
            |_| anyhow::bail!("cancelled request activated"),
        )
    })?;
    assert_eq!(queue.state(first), Some(TickingFutureState::Unavailable));

    // A validated late result belongs to the same holder, but cannot turn
    // its already-cancelled ticking request into a successful one.
    assert!(last.complete_full(&old_owner, &mut ids)?.is_none());
    last.update_highest_allowed_status(33);
    for (position, holder) in &mut holders {
        futures.insert(*position, holder.request_full(&mut ids)?);
    }
    let second = TickingRequestId(2);
    queue.start(second, center, |position| futures.get(&position).copied())?;
    queue.dispatch_next(|callback| callback.dispatch(|_| Ok(()), |_| Ok(()), |_| Ok(())))?;
    assert_eq!(queue.state(first), Some(TickingFutureState::Unavailable));
    assert_eq!(queue.state(second), Some(TickingFutureState::Available));
    Ok(())
}

#[test]
fn generation_results_are_bound_to_the_original_holder() -> Result<()> {
    let mut ids = FullFutureIds::default();
    let original = FullGenerationHolder::default();
    let old_owner = original.owner();
    let mut replacement = FullGenerationHolder::default();
    replacement.update_highest_allowed_status(33);
    let pending = replacement.request_full(&mut ids)?;
    assert!(replacement.complete_full(&old_owner, &mut ids).is_err());
    assert_eq!(replacement.request_full(&mut ids)?, pending);
    let owner = replacement.owner();
    assert_eq!(
        replacement.complete_full(&owner, &mut ids)?,
        Some((pending.id, FullFutureResult::Full))
    );
    Ok(())
}

#[test]
#[cfg(target_os = "windows")]
#[ignore = "requires installed Minecraft Java Edition 26.3 and Java 25 JDK"]
fn full_generation_holder_matches_java_26_3() -> Result<()> {
    let expected = crate::worldgen::postprocessing_java_results(
        "FullGenerationHolderOracle",
        include_str!("../tests/java/FullGenerationHolderOracle.java"),
        &[],
    );
    assert_eq!(lifecycle_trace()?, expected);
    Ok(())
}
