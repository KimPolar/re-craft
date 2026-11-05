//! Buried treasure placement in an unpublished generation region.
use super::*;
use native_core::beardifier::BoundingBox;
use native_resources::{
    monster_room::MonsterRoomWorld,
    structure_piece::{BuriedTreasureOutcome, TreasureWorld, place_buried_treasure},
    structure_start::{HorizontalDirection, SimpleStructureStart},
};

fn as_value(state: &BlockState) -> Value {
    if state.properties.is_empty() {
        Value::String(state.name.clone())
    } else {
        serde_json::json!({"id": state.name, "properties": state.properties})
    }
}

fn as_state(value: &Value) -> Result<BlockState> {
    let name = value
        .as_str()
        .or_else(|| value.get("id").and_then(Value::as_str))
        .context("treasure block has no identifier")?;
    let id = ResourceId::parse(name)?;
    let mut properties = BTreeMap::new();
    if let Some(value) = value.get("properties") {
        for (key, property) in value
            .as_object()
            .context("invalid treasure block properties")?
        {
            properties.insert(
                key.clone(),
                property
                    .as_str()
                    .context("invalid treasure property")?
                    .to_owned(),
            );
        }
    }
    Ok(BlockState {
        name: format!("{}:{}", id.namespace, id.path),
        properties,
    })
}

impl DecorationRegion<'_> {
    /// Place one structure piece in the unpublished region. The caller must
    /// select the Java decoration step, start order, seed and writable box.
    /// On failure the region must be discarded; the piece and RNG stay intact.
    pub fn place_buried_treasure(
        &mut self,
        piece: &mut SimpleStructureStart,
        writable: BoundingBox,
        random: &mut DecorationRandom,
    ) -> Result<BuriedTreasureOutcome> {
        ensure!(!self.failed, "generation attempt previously failed");
        let mut staged_piece = piece.clone();
        let mut staged_random = random.clone();
        self.failed = true;
        let result = place_buried_treasure(&mut staged_piece, self, writable, &mut staged_random)?;
        *piece = staged_piece;
        *random = staged_random;
        self.failed = false;
        Ok(result)
    }
}

impl TreasureWorld for DecorationRegion<'_> {
    fn vertical_bounds(&self) -> (i32, i32) {
        (
            self.target.min_y,
            self.target.min_y + self.target.height - 1,
        )
    }

    fn ocean_floor_height(&mut self, x: i32, z: i32) -> Result<i32> {
        OreVeinWorld::ocean_floor_height(self, x, z)
    }

    fn block(&mut self, position: [i32; 3]) -> Result<Value> {
        Ok(as_value(&OreWorld::block(self, position)?))
    }

    fn is_air(&mut self, value: &Value) -> Result<bool> {
        Ok(super::is_air(&as_state(value)?))
    }

    fn is_solid_render(&mut self, value: &Value) -> Result<bool> {
        Ok(monster_block_flags(&as_state(value)?)?.1)
    }

    fn set_block_and_update(&mut self, position: [i32; 3], value: &Value) -> Result<()> {
        if !self.writable.contains(&chunk(position)) || self.is_outside(position[1]) {
            return Ok(());
        }
        let old = self.read(position)?;
        monster_block_flags(&old)?;
        ensure!(
            is_air(&old) || matches!(old.name.as_str(), "minecraft:water" | "minecraft:lava"),
            "unsupported treasure replacement source"
        );
        let state = as_state(value)?;
        monster_block_flags(&state)?;
        ensure!(
            !is_air(&state)
                && !matches!(
                    state.name.as_str(),
                    "minecraft:water" | "minecraft:lava" | "minecraft:spawner"
                ),
            "unsupported treasure replacement effects: {}",
            state.name
        );
        self.set_proto_block(position, &state)?;
        if state.name == "minecraft:chest" {
            ensure!(
                !self.monster_entities.contains_key(&position),
                "existing block entity cannot be replaced"
            );
            Arc::make_mut(&mut self.monster_entities).insert(
                position,
                Some(super::super::GeneratedMonsterRoomEntity::PendingChest { position, state }),
            );
            Arc::make_mut(self.pending_order.entry(chunk(position)).or_default()).insert(position);
        }
        Ok(())
    }

    fn set_chest(&mut self, position: [i32; 3], facing: HorizontalDirection) -> Result<bool> {
        ensure!(
            self.writable.contains(&chunk(position)) && !self.is_outside(position[1]),
            "treasure chest is outside the supported writable region"
        );
        MonsterRoomWorld::set_block(self, position, RoomBlock::Chest(facing))?;
        MonsterRoomWorld::chest_entity_exists(self, position)
    }

    fn set_chest_loot(&mut self, position: [i32; 3], table: &str, seed: i64) -> Result<()> {
        ensure!(
            table == "minecraft:chests/buried_treasure",
            "unexpected treasure loot table"
        );
        let Some(Some(super::super::GeneratedMonsterRoomEntity::Chest(chest))) =
            self.monster_entities.get(&position)
        else {
            bail!("generated treasure chest is unavailable")
        };
        let chest = super::super::GeneratedLootChest::new(position, chest.facing, table, seed)?;
        Arc::make_mut(&mut self.monster_entities).insert(
            position,
            Some(super::super::GeneratedMonsterRoomEntity::Chest(chest)),
        );
        Ok(())
    }
}
