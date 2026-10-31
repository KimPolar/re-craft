//! Vanilla 26.3 stair item IDs and their ordinary stack behavior.

pub(crate) use native_core::stairs::ITEMS;

pub(crate) fn is_stair_item(id: &str) -> bool {
    id.strip_prefix("minecraft:")
        .is_some_and(|name| ITEMS.binary_search(&name).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stair_ids_are_sorted_unique_and_namespaced() {
        assert!(ITEMS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(ITEMS.len(), 97);
        assert!(is_stair_item("minecraft:oak_stairs"));
        assert!(!is_stair_item("example:oak_stairs"));
        assert!(!is_stair_item("minecraft:unknown_stairs"));
    }
}
