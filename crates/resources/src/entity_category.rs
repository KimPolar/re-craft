//! Read entity spawn categories from the verified 26.3 client registration code.
use anyhow::{Context, Result, bail, ensure};
use std::collections::BTreeMap;

use crate::{item_registry::Class, packs::ResourceId};

const TYPES: &str = "net/minecraft/world/entity/EntityTypes";
const KEYS: &str = "net/minecraft/world/entity/EntityTypeIds";
const CATEGORY: &str = "net/minecraft/world/entity/MobCategory";
const TYPE_DESCRIPTOR: &str = "Lnet/minecraft/world/entity/EntityType;";
const KEY_DESCRIPTOR: &str = "Lnet/minecraft/resources/ResourceKey;";
const CATEGORY_DESCRIPTOR: &str = "Lnet/minecraft/world/entity/MobCategory;";
const BUILDER: &str = "net/minecraft/world/entity/EntityType$Builder";
const REGISTER: &str = "(Lnet/minecraft/resources/ResourceKey;Lnet/minecraft/world/entity/EntityType$Builder;)Lnet/minecraft/world/entity/EntityType;";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityCategory {
    Monster,
    Creature,
    Ambient,
    Axolotls,
    UndergroundWaterCreature,
    WaterCreature,
    WaterAmbient,
    Misc,
}

impl EntityCategory {
    fn parse(value: &str) -> Result<Self> {
        Ok(match value {
            "MONSTER" => Self::Monster,
            "CREATURE" => Self::Creature,
            "AMBIENT" => Self::Ambient,
            "AXOLOTLS" => Self::Axolotls,
            "UNDERGROUND_WATER_CREATURE" => Self::UndergroundWaterCreature,
            "WATER_CREATURE" => Self::WaterCreature,
            "WATER_AMBIENT" => Self::WaterAmbient,
            "MISC" => Self::Misc,
            _ => bail!("unsupported entity category {value}"),
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monster => "monster",
            Self::Creature => "creature",
            Self::Ambient => "ambient",
            Self::Axolotls => "axolotls",
            Self::UndergroundWaterCreature => "underground_water_creature",
            Self::WaterCreature => "water_creature",
            Self::WaterAmbient => "water_ambient",
            Self::Misc => "misc",
        }
    }
}

/// Contains only categories proven by the hash-verified client's initializer.
#[derive(Clone, Debug)]
pub struct EntityCategoryRegistry {
    categories: BTreeMap<String, EntityCategory>,
}

impl EntityCategoryRegistry {
    pub fn load(mut read_class: impl FnMut(&str) -> Result<Option<Vec<u8>>>) -> Result<Self> {
        let bytes = read_class(&format!("{TYPES}.class"))?
            .context("entity type registration class is missing")?;
        let class = Class::read(&bytes).context("invalid entity type registration class")?;
        ensure!(
            class.name == TYPES,
            "unexpected entity type registration class"
        );
        let mut categories = BTreeMap::new();
        let mut key: Option<String> = None;
        let mut category = None;
        let mut builder_seen = false;
        let mut registered = false;
        let mut previous_was_category = false;
        for ins in class.entity_type_instructions()? {
            let mut current_is_category = false;
            if matches!(ins.op, 0xb2 | 0xb3 | 0xb8) {
                let (owner, name, descriptor) =
                    class.member(ins.index.context("missing member reference")?)?;
                match ins.op {
                    0xb2 if owner == KEYS && descriptor == KEY_DESCRIPTOR => {
                        ensure!(
                            key.is_none() && category.is_none() && !builder_seen && !registered,
                            "ambiguous entity registration key"
                        );
                        ensure!(
                            !name.is_empty()
                                && name.bytes().all(|c| c.is_ascii_uppercase()
                                    || c.is_ascii_digit()
                                    || c == b'_'),
                            "invalid entity registration name"
                        );
                        key = Some(name.to_owned());
                    }
                    0xb2 if owner == CATEGORY && descriptor == CATEGORY_DESCRIPTOR => {
                        ensure!(
                            key.is_some() && category.is_none() && !builder_seen,
                            "ambiguous entity category"
                        );
                        category = Some(EntityCategory::parse(name)?);
                        current_is_category = true;
                    }
                    0xb8 if owner == BUILDER && (name == "of" || name == "createNothing") => {
                        let expected = if name == "of" {
                            "(Lnet/minecraft/world/entity/EntityType$EntityFactory;Lnet/minecraft/world/entity/MobCategory;)Lnet/minecraft/world/entity/EntityType$Builder;"
                        } else {
                            "(Lnet/minecraft/world/entity/MobCategory;)Lnet/minecraft/world/entity/EntityType$Builder;"
                        };
                        ensure!(
                            descriptor == expected
                                && previous_was_category
                                && key.is_some()
                                && category.is_some()
                                && !builder_seen,
                            "unsupported entity builder"
                        );
                        builder_seen = true;
                    }
                    0xb8 if owner == TYPES && name == "register" => {
                        ensure!(
                            descriptor == REGISTER
                                && key.is_some()
                                && category.is_some()
                                && builder_seen
                                && !registered,
                            "unsupported entity registration"
                        );
                        registered = true;
                    }
                    0xb3 if owner == TYPES && descriptor == TYPE_DESCRIPTOR => {
                        ensure!(
                            registered && key.as_deref() == Some(name),
                            "entity field does not match its registration"
                        );
                        let name = key
                            .take()
                            .expect("validated entity key")
                            .to_ascii_lowercase();
                        let id = ResourceId::parse(&format!("minecraft:{name}"))?;
                        let id = format!("{}:{}", id.namespace, id.path);
                        ensure!(
                            categories
                                .insert(id, category.take().context("missing entity category")?)
                                .is_none(),
                            "duplicate entity registration"
                        );
                        builder_seen = false;
                        registered = false;
                    }
                    _ => {}
                }
            }
            previous_was_category = current_is_category;
        }
        ensure!(
            key.is_none() && category.is_none() && !builder_seen && !registered,
            "incomplete entity registration"
        );
        ensure!(!categories.is_empty(), "entity category registry is empty");
        Ok(Self { categories })
    }

    pub fn category(&self, id: &ResourceId) -> Option<EntityCategory> {
        self.categories
            .get(&format!("{}:{}", id.namespace, id.path))
            .copied()
    }

    /// Suitable for CreatureSpawnLayer: MISC types use Java's pig fallback.
    pub fn is_misc(&self, id: &ResourceId) -> Option<bool> {
        self.category(id)
            .map(|category| category == EntityCategory::Misc)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, EntityCategory)> {
        self.categories
            .iter()
            .map(|(id, category)| (id.as_str(), *category))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_malformed_registration_is_rejected() {
        assert!(EntityCategoryRegistry::load(|_| Ok(None)).is_err());
        assert!(EntityCategoryRegistry::load(|_| Ok(Some(vec![0xca, 0xfe, 0xba, 0xbe]))).is_err());
    }
}
