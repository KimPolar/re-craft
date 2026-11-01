//! Replay fixed-seed chest loot before authorizing an inventory transaction.
use crate::nbt::{Tag, TagType};
use anyhow::{Context, Result, bail, ensure};
use native_resources::{
    chest_loot::{ChestLootItem, ChestLootTable},
    packs::ResourceId,
};

/// Evidence tied to the exact source entity and generated inventory.
#[derive(Clone, Debug)]
pub struct VerifiedFixedChestLoot {
    pub(crate) source: Tag,
    pub(crate) items: Tag,
}

impl VerifiedFixedChestLoot {
    /// `table` must be the active data-pack table for `table_id`; `max_stack`
    /// must validate the prototype against the active item and feature registries.
    pub fn replay(
        source: &Tag,
        table_id: &ResourceId,
        table: &ChestLootTable,
        max_stack: impl FnMut(&ChestLootItem) -> Result<u32>,
    ) -> Result<Self> {
        let Tag::Compound(fields) = source else {
            bail!("chest entity is not a compound")
        };
        let get = |name: &str| -> Result<Option<&Tag>> {
            let mut matching = fields.iter().filter(|(key, _)| key.matches(name));
            let value = matching.next().map(|(_, value)| value);
            ensure!(matching.next().is_none(), "duplicate chest field {name}");
            Ok(value)
        };
        let Some(Tag::String(id)) = get("LootTable")? else {
            bail!("chest loot table is missing")
        };
        let id = String::from_utf16(&id.0).context("invalid loot table id")?;
        ensure!(
            &ResourceId::parse(&id)? == table_id,
            "chest loot table differs"
        );
        let Some(Tag::Long(seed)) = get("LootTableSeed")? else {
            bail!("fixed chest loot seed is missing")
        };
        ensure!(*seed != 0, "zero seed requires the world random sequence");
        ensure!(get("Items")?.is_none(), "pending loot also contains items");
        let slots = table.fill_seeded(&[false; 27], *seed, max_stack)?;
        let mut items = Vec::new();
        for (slot, item) in slots.into_iter().enumerate() {
            let Some(item) = item else { continue };
            let id = format!("{}:{}", item.id.namespace, item.id.path);
            let mut fields = vec![
                ("Slot".into(), Tag::Byte(slot as i8)),
                ("id".into(), Tag::String(id.as_str().into())),
                ("count".into(), Tag::Int(i32::try_from(item.count)?)),
            ];
            if let Some(potion) = item.potion {
                let id = format!("{}:{}", potion.namespace, potion.path);
                fields.push((
                    "components".into(),
                    Tag::Compound(vec![(
                        "minecraft:potion_contents".into(),
                        Tag::Compound(vec![("potion".into(), Tag::String(id.as_str().into()))]),
                    )]),
                ));
            }
            items.push(Tag::Compound(fields));
        }
        Ok(Self {
            source: source.clone(),
            items: Tag::List {
                element_type: TagType::Compound,
                values: items,
            },
        })
    }
}
