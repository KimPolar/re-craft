//! Coordinate a server distance update with real chunk-holder operations.
//!
//! This does not manufacture holders or FULL futures. The driver must own the
//! actual loading/generation state and supply stable identities for its live
//! holders. A failed update is poisoned because earlier callbacks may already
//! have changed the world or queued work.

use std::{
    collections::HashMap,
    sync::{Arc, Weak},
};

use anyhow::{Result, ensure};

use crate::{
    loading_chunks::LoadingLevelChange,
    simulation_player::{PlayerLoadingRelease, PlayerSimulation},
};

/// One live holder identity and the identity hash used for its update set.
/// `id` must distinguish different holder instances, including new epochs at
/// the same chunk position. The identity hash must remain stable for that id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HolderIdentity {
    pub id: u64,
    pub identity_hash: u32,
}

/// A unique identity for one live chunk-map driver. Keep it with the driver;
/// another world or replacement driver must create a new identity.
#[derive(Default)]
pub struct DistanceDriverIdentity(Arc<()>);

/// Operations owned by the running chunk map and its generation executor.
pub trait DistanceUpdateDriver {
    fn identity(&self) -> &DistanceDriverIdentity;

    /// Apply this level change to the real holder map. `change.previous` is
    /// the graph's old level; a dropping holder can have a different current
    /// ticket level, so the driver must inspect its own holder map. Return the
    /// holder that needs its futures updated, or None if there is no holder.
    fn schedule(&mut self, change: LoadingLevelChange) -> Result<Option<HolderIdentity>>;

    /// First pass over the changed holders. This updates generation status
    /// before any holder starts its ticking futures.
    fn update_highest_allowed_status(&mut self, holder: HolderIdentity) -> Result<()>;

    /// Second pass over the same holders in the same order.
    fn update_futures(&mut self, holder: HolderIdentity) -> Result<()>;

    /// Attach the token to the current holder's actual entity-ticking future.
    fn register_player_loading_release(&mut self, token: PlayerLoadingRelease) -> Result<()>;
}

/// fastutil's ReferenceOpenHashSet uses identity hashes and iterates the
/// occupied table in reverse. Keeping its capacity on clear affects the next
/// update. Native holder hashes need not equal another JVM's identity hashes.
struct HolderSet {
    slots: Vec<Option<HolderIdentity>>,
    hashes: HashMap<u64, u32>,
}

impl Default for HolderSet {
    fn default() -> Self {
        Self {
            slots: vec![None; 32],
            hashes: HashMap::new(),
        }
    }
}

impl HolderSet {
    fn slot(&self, holder: HolderIdentity) -> usize {
        let mixed = holder.identity_hash.wrapping_mul(0x9e37_79b9);
        let mut index = (mixed ^ (mixed >> 16)) as usize & (self.slots.len() - 1);
        while self.slots[index].is_some_and(|other| other.id != holder.id) {
            index = (index + 1) & (self.slots.len() - 1);
        }
        index
    }

    fn insert(&mut self, holder: HolderIdentity) -> Result<()> {
        if let Some(&hash) = self.hashes.get(&holder.id) {
            ensure!(hash == holder.identity_hash, "holder identity hash changed");
            return Ok(());
        }
        let index = self.slot(holder);
        self.slots[index] = Some(holder);
        self.hashes.insert(holder.id, holder.identity_hash);
        // ReferenceOpenHashSet rehashes after adding the entry whose previous
        // size reached maxFill (three quarters of the table).
        if self.hashes.len() > self.slots.len() * 3 / 4 {
            let new_size = self.slots.len() * 2;
            let old = std::mem::replace(&mut self.slots, vec![None; new_size]);
            for holder in old.into_iter().rev().flatten() {
                let index = self.slot(holder);
                self.slots[index] = Some(holder);
            }
        }
        Ok(())
    }

    fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    fn ordered(&self) -> impl Iterator<Item = HolderIdentity> + '_ {
        self.slots.iter().rev().flatten().copied()
    }

    fn clear(&mut self) {
        self.slots.fill(None);
        self.hashes.clear();
    }
}

/// The changed-holder set and the fail-closed state of one distance manager.
#[derive(Default)]
pub struct DistanceUpdates {
    changed: HolderSet,
    failed: bool,
    player_identity: Option<Weak<()>>,
    driver_identity: Option<Weak<()>>,
}

impl DistanceUpdates {
    /// Include a holder changed by another chunk-map operation before the next
    /// distance update. The same live holder is updated only once per pass.
    pub fn mark_holder_changed(&mut self, holder: HolderIdentity) -> Result<()> {
        ensure!(!self.failed, "distance update coordinator has failed");
        self.changed.insert(holder)
    }

    /// Run the same high-level phases as 26.3 DistanceManager.runAllUpdates.
    /// The return value asks the server to run another distance update.
    /// Dispatcher and main-executor work remain separately stepped by the
    /// caller, so they are not silently reordered around holder callbacks.
    pub fn run(
        &mut self,
        player: &mut PlayerSimulation,
        driver: &mut impl DistanceUpdateDriver,
    ) -> Result<bool> {
        ensure!(!self.failed, "distance update coordinator has failed");
        let player_identity = player
            .loading_identity()
            .ok_or_else(|| anyhow::anyhow!("loading-ticket tracking is not enabled"))?;
        let driver_identity = &driver.identity().0;
        match (&self.player_identity, &self.driver_identity) {
            (Some(player_owner), Some(driver_owner)) => {
                ensure!(
                    Weak::ptr_eq(player_owner, &Arc::downgrade(player_identity))
                        && Weak::ptr_eq(driver_owner, &Arc::downgrade(driver_identity)),
                    "distance update coordinator belongs to another session or driver"
                );
            }
            (None, None) => {
                self.player_identity = Some(Arc::downgrade(player_identity));
                self.driver_identity = Some(Arc::downgrade(driver_identity));
            }
            _ => unreachable!("distance update owners are set together"),
        }
        player.run_updates();
        let mut failure = None;
        let budget = i32::MAX as usize;
        let remaining = player
            .run_loading_updates(budget, |change| {
                if failure.is_some() {
                    return;
                }
                match driver.schedule(change) {
                    Ok(Some(holder)) => {
                        if let Err(error) = self.changed.insert(holder) {
                            failure = Some(error);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => failure = Some(error),
                }
            })
            .expect("loading tracker was checked");
        if let Some(error) = failure {
            self.failed = true;
            return Err(error);
        }
        if !self.changed.is_empty() {
            for holder in self.changed.ordered() {
                if let Err(error) = driver.update_highest_allowed_status(holder) {
                    self.failed = true;
                    return Err(error);
                }
            }
            for holder in self.changed.ordered() {
                if let Err(error) = driver.update_futures(holder) {
                    self.failed = true;
                    return Err(error);
                }
            }
            self.changed.clear();
            return Ok(true);
        }
        if let Err(error) = player
            .register_player_loading_releases(|token| driver.register_player_loading_release(token))
        {
            self.failed = true;
            return Err(error);
        }
        Ok(remaining != budget)
    }
}

#[cfg(test)]
#[path = "distance_updates_tests.rs"]
mod tests;
