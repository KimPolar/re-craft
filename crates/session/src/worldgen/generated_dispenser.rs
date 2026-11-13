//! Deferred dispenser contents produced by structure templates.
use anyhow::{Context, Result, bail, ensure};
use native_persistence::nbt::{Tag, TagType};
use native_resources::packs::ResourceId;

#[derive(Clone, Debug)]
pub struct GeneratedDispenser {
    pub position: [i32; 3],
    pub state: native_world::loaded::BlockState,
    loot: Option<(String, i64)>,
}

impl GeneratedDispenser {
    pub fn from_template(
        position: [i32; 3],
        state: native_world::loaded::BlockState,
        nbt: &Tag,
        seed: i64,
    ) -> Result<Self> {
        ensure!(
            native_world::collision::dispenser_state(&state),
            "invalid dispenser state"
        );
        let Tag::Compound(fields) = nbt else {
            bail!("dispenser data is not a compound")
        };
        let mut keys = std::collections::BTreeSet::new();
        ensure!(
            fields
                .iter()
                .all(|(key, _)| ["id", "LootTable", "LootTableSeed", "Items"]
                    .iter()
                    .any(|name| key.matches(name))
                    && keys.insert(key.0.clone())),
            "unsupported dispenser data"
        );
        ensure!(
            matches!(nbt.get("id"), Some(Tag::String(id)) if id.matches("minecraft:dispenser")),
            "invalid dispenser entity id"
        );
        ensure!(
            matches!(nbt.get("LootTableSeed"), None | Some(Tag::Long(_))),
            "invalid dispenser loot seed"
        );
        let loot = match nbt.get("LootTable") {
            Some(Tag::String(table)) => {
                ensure!(
                    nbt.get("Items").is_none(),
                    "dispenser loot and inventory cannot both be preserved"
                );
                let table = String::from_utf16(&table.0).context("invalid dispenser loot table")?;
                ResourceId::parse(&table)?;
                Some((table, seed))
            }
            None => {
                let empty = match nbt.get("Items") {
                    None => true,
                    Some(Tag::List {
                        element_type: TagType::End | TagType::Compound,
                        values,
                    }) => values.is_empty(),
                    _ => false,
                };
                ensure!(empty, "nonempty dispenser inventory is not yet supported");
                None
            }
            _ => bail!("invalid dispenser loot table"),
        };
        Ok(Self {
            position,
            state,
            loot,
        })
    }

    pub fn block_entity(&self) -> Tag {
        let [x, y, z] = self.position;
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:dispenser".into())),
            ("x".into(), Tag::Int(x)),
            ("y".into(), Tag::Int(y)),
            ("z".into(), Tag::Int(z)),
            ("components".into(), Tag::Compound(vec![])),
        ];
        if let Some((table, seed)) = &self.loot {
            fields.push(("LootTable".into(), Tag::String(table.as_str().into())));
            if *seed != 0 {
                fields.push(("LootTableSeed".into(), Tag::Long(*seed)));
            }
        } else {
            fields.push((
                "Items".into(),
                Tag::List {
                    element_type: TagType::End,
                    values: vec![],
                },
            ));
        }
        Tag::Compound(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn state() -> native_world::loaded::BlockState {
        native_world::loaded::BlockState {
            name: "minecraft:dispenser".into(),
            properties: BTreeMap::from([
                ("facing".into(), "west".into()),
                ("triggered".into(), "false".into()),
            ]),
        }
    }

    #[test]
    fn preserves_loot_and_rejects_unhandled_inventory() -> Result<()> {
        let table = "minecraft:dispensers/trial_chambers/chamber";
        let nbt = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:dispenser".into())),
            ("LootTable".into(), Tag::String(table.into())),
            ("LootTableSeed".into(), Tag::Long(7)),
        ]);
        let generated = GeneratedDispenser::from_template([3, 7, -2], state(), &nbt, 42)?;
        let saved = generated.block_entity();
        assert!(matches!(saved.get("LootTable"), Some(Tag::String(value)) if value.matches(table)));
        assert!(matches!(saved.get("LootTableSeed"), Some(Tag::Long(42))));
        let zero_seed = GeneratedDispenser::from_template([3, 7, -2], state(), &nbt, 0)?;
        assert!(zero_seed.block_entity().get("LootTableSeed").is_none());
        let mut invalid_state = state();
        invalid_state
            .properties
            .insert("facing".into(), "sideways".into());
        assert!(GeneratedDispenser::from_template([3, 7, -2], invalid_state, &nbt, 42).is_err());
        let Tag::Compound(mut fields) = nbt else {
            unreachable!()
        };
        fields.push((
            "Items".into(),
            Tag::List {
                element_type: TagType::Compound,
                values: vec![Tag::Compound(vec![])],
            },
        ));
        assert!(
            GeneratedDispenser::from_template([3, 7, -2], state(), &Tag::Compound(fields), 42)
                .is_err()
        );
        Ok(())
    }
}
