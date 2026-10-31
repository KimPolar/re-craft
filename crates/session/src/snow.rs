//! Snow-layer behavior shared by the session and Java comparisons.

/// SnowLayerBlock.randomTick considers only block light, not sky light.
pub fn melts_in_block_light(light: u8) -> bool {
    light > 11
}
