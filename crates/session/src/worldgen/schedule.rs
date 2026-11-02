//! Java 26.3 chunk-generation dependencies and a deterministic work plan.
//! Completing a plan entry only records an already successful generation step.
use anyhow::{Context, Result, ensure};
use native_world::loaded::ChunkPos;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum GenerationStage {
    Empty,
    StructureStarts,
    StructureReferences,
    Biomes,
    Terrain,
    Features,
    InitializeLight,
    Light,
    Spawn,
    Full,
}
use GenerationStage::*;
impl GenerationStage {
    pub const ALL: [Self; 10] = [
        Empty,
        StructureStarts,
        StructureReferences,
        Biomes,
        Terrain,
        Features,
        InitializeLight,
        Light,
        Spawn,
        Full,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Empty => "minecraft:empty",
            StructureStarts => "minecraft:structure_starts",
            StructureReferences => "minecraft:structure_references",
            Biomes => "minecraft:biomes",
            Terrain => "minecraft:terrain",
            Features => "minecraft:features",
            InitializeLight => "minecraft:initialize_light",
            Light => "minecraft:light",
            Spawn => "minecraft:spawn",
            Full => "minecraft:full",
        }
    }
    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self as usize + 1).copied()
    }
    pub fn previous(self) -> Option<Self> {
        (self as usize)
            .checked_sub(1)
            .and_then(|n| Self::ALL.get(n))
            .copied()
    }
    /// Dependency at each Chebyshev distance, beginning with the center.
    pub fn direct(self) -> &'static [Self] {
        match self {
            Empty => &[],
            StructureStarts => &[Empty],
            StructureReferences => &[StructureStarts; 9],
            Biomes => &[
                StructureReferences,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Terrain => &[
                Biomes,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Features => &[
                Terrain,
                Terrain,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            InitializeLight => &[Features],
            Light => &[InitializeLight, InitializeLight],
            Spawn => &[Light, Biomes],
            Full => &[Spawn],
        }
    }
    pub fn accumulated(self) -> &'static [Self] {
        match self {
            Empty => &[],
            StructureStarts => &[Empty],
            StructureReferences => &[StructureStarts; 9],
            Biomes => &[
                StructureReferences,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Terrain => &[
                Biomes,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Features => &[
                Terrain,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            InitializeLight => &[
                Features,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Light => &[
                InitializeLight,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Spawn => &[
                Light,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
            Full => &[
                Spawn,
                InitializeLight,
                Terrain,
                Biomes,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
                StructureStarts,
            ],
        }
    }
    pub fn block_write_radius(self) -> Option<i32> {
        match self {
            Terrain => Some(0),
            Features => Some(1),
            _ => None,
        }
    }
}

/// Single-threaded plan. A caller may record a step only after performing the
/// actual operation; this planner does not generate or save chunk contents.
#[derive(Default)]
pub struct GenerationSchedule {
    required: BTreeMap<ChunkPos, GenerationStage>,
    completed: BTreeMap<ChunkPos, GenerationStage>,
}

impl GenerationSchedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Expand Java's accumulated dependencies transactionally. Requests can overlap or
    /// increase existing targets without discarding completed work.
    pub fn request(&mut self, pos: ChunkPos, target: GenerationStage) -> Result<()> {
        let mut staged = BTreeMap::from([(pos, target)]);
        for (radius, &needed) in target.accumulated().iter().enumerate().skip(1) {
            let r = i32::try_from(radius)?;
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    let neighbor = ChunkPos {
                        x: pos
                            .x
                            .checked_add(dx)
                            .context("generation chunk X overflow")?,
                        z: pos
                            .z
                            .checked_add(dz)
                            .context("generation chunk Z overflow")?,
                    };
                    staged.insert(neighbor, needed);
                }
            }
        }
        for (pos, needed) in staged {
            self.required
                .entry(pos)
                .and_modify(|current| *current = (*current).max(needed))
                .or_insert(needed);
        }
        Ok(())
    }
    pub fn required(&self, pos: ChunkPos) -> Option<GenerationStage> {
        self.required.get(&pos).copied()
    }
    pub fn completed(&self, pos: ChunkPos) -> Option<GenerationStage> {
        self.required
            .contains_key(&pos)
            .then(|| self.completed.get(&pos).copied().unwrap_or(Empty))
    }
    pub fn requested_chunks(&self) -> usize {
        self.required.len()
    }

    pub fn ready(&self, pos: ChunkPos, target: GenerationStage) -> bool {
        let Some(current) = self.completed(pos) else {
            return false;
        };
        if current.next() != Some(target)
            || self.required(pos).is_none_or(|required| target > required)
        {
            return false;
        }
        for (radius, &needed) in target.direct().iter().enumerate() {
            let r = radius as i32;
            for dz in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dz.abs()) != r {
                        continue;
                    }
                    let (Some(x), Some(z)) = (pos.x.checked_add(dx), pos.z.checked_add(dz)) else {
                        return false;
                    };
                    if self
                        .completed(ChunkPos { x, z })
                        .is_none_or(|actual| actual < needed)
                    {
                        return false;
                    }
                }
            }
        }
        true
    }
    pub fn next_ready(&self) -> Option<(ChunkPos, GenerationStage)> {
        self.required.iter().find_map(|(&pos, &required)| {
            let next = self.completed(pos)?.next()?;
            (next <= required && self.ready(pos, next)).then_some((pos, next))
        })
    }
    /// Call only after the corresponding operation succeeded. A failed or
    /// cancelled operation must not be recorded as complete.
    pub fn complete(&mut self, pos: ChunkPos, target: GenerationStage) -> Result<()> {
        ensure!(self.ready(pos, target), "generation step is not ready");
        self.completed.insert(pos, target);
        Ok(())
    }

    /// Apply supported decoration and record FEATURES only after it succeeds.
    pub fn run_features(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        fresh_light: Option<(
            &super::FreshGenerationLightTracker,
            &native_resources::block_registry::GeneratedBlockRegistry,
        )>,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result = match fresh_light {
            Some((tracker, registry)) => prepared.execute_with_tracked_light(
                terrain,
                structures_enabled,
                tracker,
                registry,
            )?,
            None => prepared.execute(terrain, structures_enabled)?,
        };
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Execute FEATURES using an already published staged-light revision.
    pub fn run_features_with_staged_light(
        &mut self,
        terrain: &mut super::CarvedTerrainWorld,
        prepared: &super::PreparedDecoration<'_, '_>,
        structures_enabled: bool,
        engine: &super::StagedGenerationLightEngine,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<Vec<bool>> {
        let center = prepared.center_chunk();
        ensure!(
            self.ready(center, Features),
            "features dependencies are incomplete"
        );
        let result =
            prepared.execute_with_staged_light(terrain, structures_enabled, engine, registry)?;
        self.complete(center, Features)?;
        Ok(result)
    }

    /// Calculate direct-sky sources and register sections before advancing the
    /// fresh INITIALIZE_LIGHT stage. No light propagation is performed here.
    pub fn run_light_initialization(
        &mut self,
        terrain: &super::CarvedTerrainWorld,
        center: ChunkPos,
        tracker: &mut super::FreshGenerationLightTracker,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<()> {
        ensure!(
            self.ready(center, InitializeLight),
            "light initialization dependencies are incomplete"
        );
        tracker.initialize_chunk(terrain, center, registry)?;
        self.complete(center, InitializeLight)
    }

    /// Advance LIGHT within a frozen, fully initialized generation batch.
    /// The returned snapshot does not write a chunk or make it saveable.
    pub fn run_light(
        &mut self,
        terrain: &super::CarvedTerrainWorld,
        center: ChunkPos,
        engine: &mut super::StagedGenerationLightEngine,
        registry: &native_resources::block_registry::GeneratedBlockRegistry,
    ) -> Result<native_world::generation_light::VisibleLightSnapshot> {
        ensure!(
            self.ready(center, Light),
            "light dependencies are incomplete"
        );
        let snapshot = engine.activate(terrain, center, [center], registry)?;
        self.complete(center, Light)?;
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requirements_match_accumulated_radii_for_every_stage() {
        let center = ChunkPos { x: -3, z: 4 };
        for target in GenerationStage::ALL {
            let mut schedule = GenerationSchedule::new();
            schedule.request(center, target).unwrap();
            for dz in -14i32..=14 {
                for dx in -14i32..=14 {
                    let d = dx.abs().max(dz.abs()) as usize;
                    let expected = if d == 0 {
                        Some(target)
                    } else {
                        target.accumulated().get(d).copied()
                    };
                    assert_eq!(
                        schedule.required(ChunkPos {
                            x: center.x + dx,
                            z: center.z + dz
                        }),
                        expected,
                        "{:?} {dx},{dz}",
                        target
                    );
                }
            }
        }
    }
    #[test]
    fn steps_require_neighbor_completion_and_requests_are_transactional() {
        let center = ChunkPos { x: 0, z: 0 };
        let mut plan = GenerationSchedule::new();
        plan.request(center, Features).unwrap();
        assert!(!plan.ready(center, Features));
        assert!(plan.complete(center, Features).is_err());
        let total = plan.requested_chunks();
        assert!(
            plan.request(ChunkPos { x: i32::MAX, z: 0 }, Features)
                .is_err()
        );
        assert_eq!(plan.requested_chunks(), total);
        let mut count = 0usize;
        while !plan.ready(center, Features) {
            let (pos, stage) = plan.next_ready().expect("dependency graph must progress");
            assert_ne!((pos, stage), (center, Features));
            plan.complete(pos, stage).unwrap();
            count += 1;
            assert!(count < 10_000);
        }
        plan.complete(center, Features).unwrap();
        assert_eq!(plan.completed(center), Some(Features));
        assert!(plan.complete(center, Features).is_err());
        plan.request(center, InitializeLight).unwrap();
        assert!(plan.ready(center, InitializeLight));
        plan.complete(center, InitializeLight).unwrap();
    }
}
