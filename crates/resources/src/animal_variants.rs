//! Data-pack spawn selectors for pig, cow, and chicken variants.
//!
//! The active registry is registered in Java's resource-file order; ties
//! consume a random index in that order.
use std::{cmp::Reverse, collections::BTreeSet};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use crate::{
    cat_variants::CatWorldConditions,
    packs::{ResourceId, ResourceProvider},
    worldgen_region_random::{
        AnimalBaseInitialization, AnimalGroupData, RegionRandom, finalize_animal_base,
    },
};

pub use crate::cat_variants::{CatSpawnContext as SpawnVariantContext, SpawnVariantEnvironment};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FarmAnimal {
    Pig,
    Cow,
    Chicken,
}

impl FarmAnimal {
    fn registry(self) -> &'static str {
        match self {
            Self::Pig => "pig_variant",
            Self::Cow => "cow_variant",
            Self::Chicken => "chicken_variant",
        }
    }

    fn sound_registry(self) -> &'static str {
        match self {
            Self::Pig => "pig_sound_variant",
            Self::Cow => "cow_sound_variant",
            Self::Chicken => "chicken_sound_variant",
        }
    }
}

struct Selector {
    priority: i32,
    variant: ResourceId,
    condition_key: Option<String>,
}

/// Loaded selectors in Java registry order. Multiple matching selectors for
/// the same variant remain separate entries, just as in PriorityProvider.
pub struct FarmAnimalVariants {
    biome_registry: BTreeSet<ResourceId>,
    selectors: Vec<Selector>,
    conditions: CatWorldConditions,
}

/// The values applied by a fresh pig, cow or chicken's spawn finalization.
#[derive(Clone, Debug, PartialEq)]
pub struct FarmAnimalInitialization {
    /// No match leaves the entity's construction default unchanged.
    pub variant: Option<ResourceId>,
    pub sound_variant: ResourceId,
    pub animal: AnimalBaseInitialization,
}

/// Visual and sound registries from the same active data-pack stack.
pub struct FarmAnimalSpawnVariants {
    species: FarmAnimal,
    visual: FarmAnimalVariants,
    default_visual: ResourceId,
    sounds: Vec<ResourceId>,
}

impl FarmAnimalSpawnVariants {
    /// The caller must still validate the complete registries as Java does.
    pub fn load_active(provider: &mut impl ResourceProvider, animal: FarmAnimal) -> Result<Self> {
        let ordered = provider.list_registry_data(animal.registry())?;
        let temperate = ResourceId::parse("minecraft:temperate")?;
        let default_visual = ordered
            .iter()
            .find(|id| **id == temperate)
            .or_else(|| ordered.first())
            .context("farm animal variant registry is empty")?
            .clone();
        let visual = FarmAnimalVariants::load_ordered(provider, animal.registry(), &ordered)?;
        let sounds = provider.list_registry_data(animal.sound_registry())?;
        ensure!(
            !sounds.is_empty(),
            "farm animal sound variant registry is empty"
        );
        ensure!(
            i32::try_from(sounds.len()).is_ok(),
            "too many sound variants"
        );
        Ok(Self {
            species: animal,
            visual,
            default_visual,
            sounds,
        })
    }

    pub fn species(&self) -> FarmAnimal {
        self.species
    }

    pub fn uses_moon_conditions(&self) -> bool {
        self.visual.conditions.uses_moon()
    }

    pub fn uses_structure_conditions(&self) -> bool {
        self.visual.conditions.uses_structures()
    }

    /// Java constructs these animals with temperate, or the first registry
    /// entry if a data pack removed it.
    pub fn default_variant(&self) -> &ResourceId {
        &self.default_visual
    }

    /// Match Java's variant, sound, AgeableMob and Mob random draw order.
    /// Validation failures leave the caller's random source and group intact.
    pub fn finalize(
        &self,
        biome: &ResourceId,
        random: &mut RegionRandom,
        group: &mut AnimalGroupData,
        has_follow_range_bonus: bool,
    ) -> Result<FarmAnimalInitialization> {
        self.finalize_in_context(
            &SpawnVariantContext {
                biome: Some(biome),
                moon_phase: None,
                structures_with_piece: None,
            },
            random,
            group,
            has_follow_range_bonus,
        )
    }

    /// Apply world-dependent selectors from the active pack snapshot.
    pub fn finalize_in_context(
        &self,
        context: &SpawnVariantContext<'_>,
        random: &mut RegionRandom,
        group: &mut AnimalGroupData,
        has_follow_range_bonus: bool,
    ) -> Result<FarmAnimalInitialization> {
        let mut context = *context;
        self.finalize_in_environment(&mut context, random, group, has_follow_range_bonus)
    }

    /// Evaluate world conditions lazily, preserving RNG and group on errors.
    pub fn finalize_in_environment(
        &self,
        context: &mut impl SpawnVariantEnvironment,
        random: &mut RegionRandom,
        group: &mut AnimalGroupData,
        has_follow_range_bonus: bool,
    ) -> Result<FarmAnimalInitialization> {
        let mut next_random = random.clone();
        let mut next_group = *group;
        let variant = self
            .visual
            .choose_in_environment(context, &mut next_random)?
            .cloned();
        let count = i32::try_from(self.sounds.len()).context("too many sound variants")?;
        let index = next_random
            .next_int_bound(count)
            .context("empty sound registry")? as usize;
        let animal =
            finalize_animal_base(&mut next_random, &mut next_group, has_follow_range_bonus);
        let result = FarmAnimalInitialization {
            variant,
            sound_variant: self.sounds[index].clone(),
            animal,
        };
        *random = next_random;
        *group = next_group;
        Ok(result)
    }
}

impl FarmAnimalVariants {
    /// Load the active resource-backed registry in Java's registration order.
    pub fn load_active(provider: &mut impl ResourceProvider, animal: FarmAnimal) -> Result<Self> {
        let ordered = provider.list_registry_data(animal.registry())?;
        Self::load_ordered(provider, animal.registry(), &ordered)
    }

    /// `ordered_variants` must be the full, ordered active registry. Loading
    /// stops on unsupported conditions instead of silently choosing wrongly.
    pub fn load(
        provider: &mut impl ResourceProvider,
        animal: FarmAnimal,
        ordered_variants: &[ResourceId],
    ) -> Result<Self> {
        let registry = animal.registry();
        let visible: BTreeSet<_> = provider.list_data(registry)?.into_iter().collect();
        let supplied: BTreeSet<_> = ordered_variants.iter().cloned().collect();
        ensure!(
            supplied.len() == ordered_variants.len() && supplied == visible,
            "{registry} order must contain every active variant exactly once"
        );
        Self::load_ordered(provider, animal.registry(), ordered_variants)
    }

    fn load_ordered(
        provider: &mut impl ResourceProvider,
        registry: &str,
        ordered_variants: &[ResourceId],
    ) -> Result<Self> {
        let biome_registry: BTreeSet<_> =
            provider.list_data("worldgen/biome")?.into_iter().collect();
        ensure!(!biome_registry.is_empty(), "biome registry is empty");
        let mut selectors = Vec::new();
        let mut condition_values = Vec::new();
        for id in ordered_variants {
            let path = format!("data/{}/{}/{}.json", id.namespace, registry, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing variant {path}"))?;
            let value: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("invalid variant {path}"))?;
            let object = value
                .as_object()
                .with_context(|| format!("variant {path} must be an object"))?;
            let Some(conditions) = object.get("spawn_conditions") else {
                continue;
            };
            for entry in conditions
                .as_array()
                .context("spawn_conditions must be an array")?
            {
                let entry = entry
                    .as_object()
                    .context("spawn selector must be an object")?;
                let priority = entry
                    .get("priority")
                    .and_then(Value::as_i64)
                    .and_then(|n| i32::try_from(n).ok())
                    .context("invalid spawn selector priority")?;
                if let Some(condition) = entry.get("condition") {
                    condition_values.push(condition.clone());
                }
                selectors.push(Selector {
                    priority,
                    variant: id.clone(),
                    condition_key: entry
                        .get("condition")
                        .map(serde_json::to_string)
                        .transpose()?,
                });
            }
        }
        // Stable sorting preserves registry order and selector order for ties.
        selectors.sort_by_key(|selector| Reverse(selector.priority));
        let conditions = CatWorldConditions::prepare_values(provider, &condition_values)?;
        Ok(Self {
            biome_registry,
            selectors,
            conditions,
        })
    }

    /// A missing match leaves the animal's construction default unchanged.
    /// A single match still consumes nextInt(1), as Java's getRandomSafe does.
    pub fn choose<'a>(
        &'a self,
        biome: &ResourceId,
        random: &mut RegionRandom,
    ) -> Result<Option<&'a ResourceId>> {
        self.choose_in_context(
            &SpawnVariantContext {
                biome: Some(biome),
                moon_phase: None,
                structures_with_piece: None,
            },
            random,
        )
    }

    pub fn choose_in_context<'a>(
        &'a self,
        context: &SpawnVariantContext<'_>,
        random: &mut RegionRandom,
    ) -> Result<Option<&'a ResourceId>> {
        let mut context = *context;
        self.choose_in_environment(&mut context, random)
    }

    pub fn choose_in_environment<'a>(
        &'a self,
        context: &mut impl SpawnVariantEnvironment,
        random: &mut RegionRandom,
    ) -> Result<Option<&'a ResourceId>> {
        let biome = context.biome()?;
        ensure!(
            self.biome_registry.contains(biome),
            "biome is absent from the active registry"
        );
        let mut priority = None;
        let mut matches = Vec::new();
        for selector in &self.selectors {
            if priority.is_some_and(|p| selector.priority < p) {
                break;
            }
            if selector
                .condition_key
                .as_ref()
                .map(|key| self.conditions.matches_environment(key, context))
                .transpose()?
                .unwrap_or(true)
            {
                priority = Some(selector.priority);
                matches.push(&selector.variant);
            }
        }
        if matches.is_empty() {
            return Ok(None);
        }
        let count = i32::try_from(matches.len()).context("too many matching variant selectors")?;
        let index = random
            .next_int_bound(count)
            .context("invalid variant count")? as usize;
        Ok(Some(matches[index]))
    }
}

/// Spawn data for an adult zombie nautilus created by a structure drowned.
#[derive(Clone, Debug, PartialEq)]
pub struct ZombieNautilusInitialization {
    pub variant: ResourceId,
    pub attack_cooldown: i32,
    pub animal: AnimalBaseInitialization,
}

/// Registry-backed selection for newly constructed zombie nautiluses.
pub struct ZombieNautilusSpawnVariants {
    visual: FarmAnimalVariants,
    default_visual: ResourceId,
}

impl ZombieNautilusSpawnVariants {
    pub fn uses_structure_conditions(&self) -> bool {
        self.visual.conditions.uses_structures()
    }
    pub fn load_active(provider: &mut impl ResourceProvider) -> Result<Self> {
        let registry = "zombie_nautilus_variant";
        let ordered = provider.list_registry_data(registry)?;
        let temperate = ResourceId::parse("minecraft:temperate")?;
        let default_visual = ordered
            .iter()
            .find(|id| **id == temperate)
            .or_else(|| ordered.first())
            .context("zombie nautilus variant registry is empty")?
            .clone();
        let visual = FarmAnimalVariants::load_ordered(provider, registry, &ordered)?;
        Ok(Self {
            visual,
            default_visual,
        })
    }

    /// Match the STRUCTURE spawn path with a fresh, empty group. Failed
    /// selection does not consume the caller's region random source.
    pub fn finalize_structure_spawn(
        &self,
        biome: &ResourceId,
        random: &mut RegionRandom,
    ) -> Result<ZombieNautilusInitialization> {
        self.finalize_structure_spawn_in_context(
            &SpawnVariantContext {
                biome: Some(biome),
                moon_phase: None,
                structures_with_piece: None,
            },
            random,
        )
    }

    pub fn finalize_structure_spawn_in_context(
        &self,
        context: &SpawnVariantContext<'_>,
        random: &mut RegionRandom,
    ) -> Result<ZombieNautilusInitialization> {
        let mut context = *context;
        self.finalize_structure_spawn_in_environment(&mut context, random)
    }

    pub fn finalize_structure_spawn_in_environment(
        &self,
        context: &mut impl SpawnVariantEnvironment,
        random: &mut RegionRandom,
    ) -> Result<ZombieNautilusInitialization> {
        let mut next = random.clone();
        let variant = self
            .visual
            .choose_in_environment(context, &mut next)?
            .unwrap_or(&self.default_visual)
            .clone();
        let attack_cooldown = 2400
            + next
                .next_int_bound(1201)
                .context("invalid attack cooldown bound")?;
        let animal = finalize_animal_base(&mut next, &mut AnimalGroupData::default(), false);
        let result = ZombieNautilusInitialization {
            variant,
            attack_cooldown,
            animal,
        };
        *random = next;
        Ok(result)
    }
}
