//! Java 26.3 villager type selection and load/finalization state.
use anyhow::{Context, Result, ensure};

use crate::packs::ResourceId;

/// `VillagerType.byBiome` uses the biome's registry key, not biome tags.
/// Direct holders and all unlisted keys, including custom namespaces, use plains.
pub fn type_for_biome(biome: Option<&ResourceId>) -> &'static str {
    let Some(biome) = biome.filter(|id| id.namespace == "minecraft") else {
        return "minecraft:plains";
    };
    match biome.path.as_str() {
        "badlands" | "desert" | "eroded_badlands" | "wooded_badlands" => "minecraft:desert",
        "bamboo_jungle" | "jungle" | "sparse_jungle" => "minecraft:jungle",
        "savanna_plateau" | "savanna" | "windswept_savanna" => "minecraft:savanna",
        "deep_frozen_ocean" | "frozen_ocean" | "frozen_river" | "ice_spikes" | "snowy_beach"
        | "snowy_taiga" | "snowy_plains" | "grove" | "snowy_slopes" | "frozen_peaks"
        | "jagged_peaks" => "minecraft:snow",
        "swamp" | "mangrove_swamp" => "minecraft:swamp",
        "old_growth_spruce_taiga"
        | "old_growth_pine_taiga"
        | "windswept_gravelly_hills"
        | "windswept_hills"
        | "taiga"
        | "windswept_forest" => "minecraft:taiga",
        _ => "minecraft:plains",
    }
}

/// Java marks loaded villager data finalized when the stored flag is true or
/// the complete VillagerData codec successfully decoded a value.
pub fn finalized_after_load(stored_flag: bool, data_decoded: bool) -> bool {
    stored_flag || data_decoded
}

/// The identity type and its initialization flag. The caller must load and
/// validate the complete villager data against the same active registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VillagerTypeState {
    pub kind: ResourceId,
    pub finalized: bool,
}

impl VillagerTypeState {
    /// Resolve the current biome only for an unfinalized villager. A missing
    /// selected registry entry or biome lookup error leaves the state intact.
    pub fn finalize(
        &mut self,
        biome: impl FnOnce() -> Result<Option<ResourceId>>,
        registered: impl FnOnce(&ResourceId) -> Result<bool>,
    ) -> Result<()> {
        if self.finalized {
            return Ok(());
        }
        let biome = biome().context("cannot resolve villager biome")?;
        let kind = ResourceId::parse(type_for_biome(biome.as_ref()))?;
        ensure!(
            registered(&kind)?,
            "villager type is absent from the active registry"
        );
        self.kind = kind;
        self.finalized = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalized_types_do_not_read_biome_and_failures_are_transactional() -> Result<()> {
        let mut state = VillagerTypeState {
            kind: ResourceId::parse("example:custom")?,
            finalized: true,
        };
        state.finalize(
            || anyhow::bail!("unexpected biome lookup"),
            |_| anyhow::bail!("unexpected registry lookup"),
        )?;
        assert_eq!(state.kind, ResourceId::parse("example:custom")?);
        state.finalized = false;
        let before = state.clone();
        assert!(
            state
                .finalize(
                    || Ok(Some(ResourceId::parse("minecraft:desert")?)),
                    |_| Ok(false)
                )
                .is_err()
        );
        assert_eq!(state, before);
        assert!(
            state
                .finalize(|| anyhow::bail!("missing biome"), |_| Ok(true))
                .is_err()
        );
        assert_eq!(state, before);
        state.finalize(
            || Ok(Some(ResourceId::parse("minecraft:desert")?)),
            |_| Ok(true),
        )?;
        assert!(state.finalized);
        assert_eq!(state.kind, ResourceId::parse("minecraft:desert")?);
        Ok(())
    }

    #[test]
    fn custom_keys_and_direct_holders_default_to_plains() -> Result<()> {
        assert_eq!(type_for_biome(None), "minecraft:plains");
        assert_eq!(
            type_for_biome(Some(&ResourceId::parse("example:desert")?)),
            "minecraft:plains"
        );
        assert_eq!(
            type_for_biome(Some(&ResourceId::parse("minecraft:unknown")?)),
            "minecraft:plains"
        );
        for flag in [false, true] {
            for decoded in [false, true] {
                assert_eq!(finalized_after_load(flag, decoded), flag || decoded);
            }
        }
        Ok(())
    }
}
