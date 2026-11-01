//! Replay chest loot before authorizing an inventory transaction.
use crate::nbt::{Tag, TagType};
use anyhow::{Context, Result, bail, ensure};
use native_core::random::XoroshiroRandom;
use native_resources::{
    chest_loot::{ChestLootItem, ChestLootPrototype, ChestLootTable},
    packs::ResourceId,
};

/// Evidence tied to the exact source entity and generated inventory.
#[derive(Clone, Debug)]
pub struct VerifiedChestLoot {
    pub(crate) source: Tag,
    pub(crate) items: Tag,
    pub(crate) sequence: Option<NamedSequenceProof>,
}

#[derive(Clone, Debug)]
pub(crate) struct NamedSequenceProof {
    pub(crate) id: ResourceId,
    pub(crate) world_seed: i64,
    pub(crate) before: (u64, u64),
    pub(crate) after: (u64, u64),
}

/// Compatibility name for callers replaying fixed-seed loot.
pub type VerifiedFixedChestLoot = VerifiedChestLoot;

impl VerifiedChestLoot {
    /// `table` must be the active data-pack table for `table_id`; `max_stack`
    /// must validate the prototype against the active item and feature registries.
    pub fn replay<P: Into<ChestLootPrototype>>(
        source: &Tag,
        table_id: &ResourceId,
        table: &ChestLootTable,
        max_stack: impl FnMut(&ChestLootItem) -> Result<P>,
    ) -> Result<Self> {
        Self::replay_inner(source, table_id, table, None, max_stack)
    }

    /// Replay a chest opened with a previously verified player luck value.
    pub fn replay_with_luck<P: Into<ChestLootPrototype>>(
        source: &Tag,
        table_id: &ResourceId,
        table: &ChestLootTable,
        luck: f32,
        max_stack: impl FnMut(&ChestLootItem) -> Result<P>,
    ) -> Result<Self> {
        Self::replay_inner(source, table_id, table, Some(luck), max_stack)
    }

    fn replay_inner<P: Into<ChestLootPrototype>>(
        source: &Tag,
        table_id: &ResourceId,
        table: &ChestLootTable,
        luck: Option<f32>,
        max_stack: impl FnMut(&ChestLootItem) -> Result<P>,
    ) -> Result<Self> {
        let seed = validate_source(source, table_id)?;
        ensure!(seed != 0, "zero seed requires the world random sequence");
        let slots = match luck {
            Some(luck) => table.fill_seeded_with_luck(&[false; 27], seed, luck, max_stack)?,
            None => table.fill_seeded(&[false; 27], seed, max_stack)?,
        };
        Ok(Self {
            source: source.clone(),
            items: encode_slots(slots)?,
            sequence: None,
        })
    }

    /// Replay a zero-seed table using the captured named sequence. The save
    /// transaction must connect these states to the saved sequence and seed.
    pub fn replay_named<P: Into<ChestLootPrototype>>(
        source: &Tag,
        table_id: &ResourceId,
        table: &ChestLootTable,
        world_seed: i64,
        before: (u64, u64),
        max_stack: impl FnMut(&ChestLootItem) -> Result<P>,
    ) -> Result<Self> {
        Self::replay_named_inner(source, table_id, table, world_seed, before, None, max_stack)
    }

    /// Replay a named sequence with the verified luck captured at chest opening.
    pub fn replay_named_with_luck<P: Into<ChestLootPrototype>>(
        source: &Tag,
        table_id: &ResourceId,
        table: &ChestLootTable,
        world_seed: i64,
        before: (u64, u64),
        luck: f32,
        max_stack: impl FnMut(&ChestLootItem) -> Result<P>,
    ) -> Result<Self> {
        Self::replay_named_inner(
            source,
            table_id,
            table,
            world_seed,
            before,
            Some(luck),
            max_stack,
        )
    }

    fn replay_named_inner<P: Into<ChestLootPrototype>>(
        source: &Tag,
        table_id: &ResourceId,
        table: &ChestLootTable,
        world_seed: i64,
        before: (u64, u64),
        luck: Option<f32>,
        max_stack: impl FnMut(&ChestLootItem) -> Result<P>,
    ) -> Result<Self> {
        ensure!(
            validate_source(source, table_id)? == 0,
            "named sequence requires a zero loot seed"
        );
        let id = table
            .random_sequence()
            .context("loot table has no named sequence")?
            .clone();
        let mut random = XoroshiroRandom::from_state(before.0, before.1);
        let slots = match luck {
            Some(luck) => {
                table.fill_with_random_and_luck(&[false; 27], &mut random, luck, max_stack)?
            }
            None => table.fill_with_random(&[false; 27], &mut random, max_stack)?,
        };
        Ok(Self {
            source: source.clone(),
            items: encode_slots(slots)?,
            sequence: Some(NamedSequenceProof {
                id,
                world_seed,
                before,
                after: random.state(),
            }),
        })
    }
}

fn validate_source(source: &Tag, table_id: &ResourceId) -> Result<i64> {
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
    let seed = match get("LootTableSeed")? {
        None => 0,
        Some(Tag::Long(seed)) => *seed,
        _ => bail!("invalid chest loot seed"),
    };
    ensure!(get("Items")?.is_none(), "pending loot also contains items");
    Ok(seed)
}

fn encode_slots(slots: Vec<Option<ChestLootItem>>) -> Result<Tag> {
    let mut items = Vec::new();
    for (slot, item) in slots.into_iter().enumerate() {
        let Some(item) = item else { continue };
        let id = format!("{}:{}", item.id.namespace, item.id.path);
        let mut fields = vec![
            ("Slot".into(), Tag::Byte(slot as i8)),
            ("id".into(), Tag::String(id.as_str().into())),
            ("count".into(), Tag::Int(i32::try_from(item.count)?)),
        ];
        let mut components = Vec::new();
        if let Some(potion) = item.potion {
            let id = format!("{}:{}", potion.namespace, potion.path);
            components.push((
                "minecraft:potion_contents".into(),
                Tag::Compound(vec![("potion".into(), Tag::String(id.as_str().into()))]),
            ));
        }
        if let Some(damage) = item.damage.filter(|damage| *damage != 0) {
            components.push(("minecraft:damage".into(), Tag::Int(i32::try_from(damage)?)));
        }
        if !components.is_empty() {
            fields.push(("components".into(), Tag::Compound(components)));
        }
        items.push(Tag::Compound(fields));
    }
    Ok(Tag::List {
        element_type: TagType::Compound,
        values: items,
    })
}
