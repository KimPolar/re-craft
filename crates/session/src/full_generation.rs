//! The FULL generation future owned by a chunk holder.
//!
//! A loading level controls whether a holder can expose FULL, but neither a
//! ticket nor a saved chunk by itself completes a generation future. The
//! caller still has to validate and deliver the actual generation result and
//! forward each returned completion to the ticking executor in its observed
//! order.

use std::sync::Arc;

use anyhow::{Result, ensure};

use crate::ticking_preparation::{FullFuture, FullFutureId, FullFutureResult};

/// Allocates identities for FULL futures used by one ticking executor. ID zero
/// represents Java's shared, already-unavailable generation future.
pub struct FullFutureIds {
    next: u64,
}

impl Default for FullFutureIds {
    fn default() -> Self {
        Self { next: 1 }
    }
}

impl FullFutureIds {
    fn allocate(&mut self) -> Result<FullFutureId> {
        let id = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("FULL future identity exhausted"))?;
        Ok(FullFutureId(id))
    }

    fn unavailable(&self) -> FullFuture {
        FullFuture {
            id: FullFutureId(0),
            result: Some(FullFutureResult::Unavailable),
        }
    }
}

/// Identity retained by a generation task for one holder instance. A later
/// holder at the same coordinates cannot accept this holder's result.
#[derive(Clone)]
pub struct FullGenerationOwner(Arc<()>);

/// State of one holder's FULL generation future. New holders initially have
/// no allowed status, even if their ticket level has already been assigned.
pub struct FullGenerationHolder {
    owner: Arc<()>,
    allowed: bool,
    full: Option<FullFuture>,
}

impl Default for FullGenerationHolder {
    fn default() -> Self {
        Self {
            owner: Arc::new(()),
            allowed: false,
            full: None,
        }
    }
}

impl FullGenerationHolder {
    pub fn owner(&self) -> FullGenerationOwner {
        FullGenerationOwner(Arc::clone(&self.owner))
    }

    /// Whether the last applied holder status permits requesting FULL.
    pub fn allows_full(&self) -> bool {
        self.allowed
    }

    pub(crate) fn owns(&self, owner: &FullGenerationOwner) -> bool {
        Arc::ptr_eq(&self.owner, &owner.0)
    }

    pub(crate) fn has_full_future(&self) -> bool {
        self.full.is_some()
    }

    pub(crate) fn full_result(&self) -> Option<FullFutureResult> {
        if !self.allowed {
            return None;
        }
        self.full?.result
    }

    /// Apply the holder's ticket level during the highest-status pass. A
    /// pending future is completed as unavailable and removed when FULL is no
    /// longer allowed. Already completed FULL futures remain in the holder.
    ///
    /// Forward a returned completion before dispatching later executor work.
    pub fn update_highest_allowed_status(
        &mut self,
        ticket_level: i32,
    ) -> Option<(FullFutureId, FullFutureResult)> {
        // ChunkLevel.generationStatus subtracts 33 using Java int arithmetic.
        let allowed = ticket_level.wrapping_sub(33) <= 0;
        let demoted = self.allowed && !allowed;
        self.allowed = allowed;
        if demoted && self.full.is_some_and(|future| future.result.is_none()) {
            let future = self.full.take().expect("pending FULL future");
            return Some((future.id, FullFutureResult::Unavailable));
        }
        None
    }

    /// Return this holder's current FULL future, creating it when required.
    /// This only requests generation: it does not complete or start I/O.
    pub fn request_full(&mut self, ids: &mut FullFutureIds) -> Result<FullFuture> {
        if !self.allowed {
            return Ok(ids.unavailable());
        }
        if let Some(future) = self.full {
            return Ok(future);
        }
        let future = FullFuture {
            id: ids.allocate()?,
            result: None,
        };
        self.full = Some(future);
        Ok(future)
    }

    /// Publish a validated FULL result from this holder's generation task.
    /// If the previous pending future was cancelled, Java's late completion
    /// fills the holder's current FULL slot, which may be a later request.
    /// The caller must independently validate the task's world and inputs.
    ///
    /// A returned completion must be delivered to the shared ticking executor;
    /// `None` means no future existed when the result was installed.
    pub fn complete_full(
        &mut self,
        owner: &FullGenerationOwner,
        ids: &mut FullFutureIds,
    ) -> Result<Option<(FullFutureId, FullFutureResult)>> {
        ensure!(
            self.owns(owner),
            "FULL generation result belongs to another holder"
        );
        if let Some(future) = &mut self.full {
            ensure!(future.result.is_none(), "FULL generation already completed");
            future.result = Some(FullFutureResult::Full);
            return Ok(Some((future.id, FullFutureResult::Full)));
        }
        self.full = Some(FullFuture {
            id: ids.allocate()?,
            result: Some(FullFutureResult::Full),
        });
        Ok(None)
    }
}

#[cfg(test)]
#[path = "full_generation_tests.rs"]
mod tests;
