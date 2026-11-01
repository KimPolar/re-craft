//! The built-in 26.3 weather attribute layers, sampled at a client tick.

fn lerp<const N: usize>(base: [f32; N], target: [f32; N], amount: f32) -> [f32; N] {
    std::array::from_fn(|i| base[i] + amount * (target[i] - base[i]))
}

fn gray(base: [f32; 3], brightness: f32, factor: f32) -> [f32; 3] {
    let gray = ((base[0] * 0.3 + base[1] * 0.59 + base[2] * 0.11) * brightness).clamp(0.0, 1.0);
    lerp(base, [gray; 3], factor)
}

fn sky_layer(base: [f32; 3], intensity: f32, brightness: f32, factor: f32) -> [f32; 3] {
    if intensity > 0.0 {
        lerp(base, gray(base, brightness, factor), intensity)
    } else {
        base
    }
}

fn multiply_layer<const N: usize>(
    base: [f32; N],
    intensity: f32,
    multiplier: [f32; N],
) -> [f32; N] {
    if intensity > 0.0 {
        lerp(
            base,
            std::array::from_fn(|i| base[i] * multiplier[i]),
            intensity,
        )
    } else {
        base
    }
}

/// SKY_COLOR: the rain layer precedes the thunder layer. `thunder` is
/// Level.getThunderLevel(1), which already includes rain strength.
pub fn sky(base: [f32; 3], rain: f32, thunder: f32) -> [f32; 3] {
    let color = sky_layer(base, rain - thunder, 0.6, 0.75);
    sky_layer(color, thunder, 0.24, 0.94)
}

/// FOG_COLOR uses the 8-bit factors built by ARGB.colorFromFloat.
pub fn fog(base: [f32; 3], rain: f32, thunder: f32) -> [f32; 3] {
    let color = multiply_layer(
        base,
        rain - thunder,
        [127.0 / 255.0, 127.0 / 255.0, 153.0 / 255.0],
    );
    multiply_layer(color, thunder, [63.0 / 255.0, 63.0 / 255.0, 76.0 / 255.0])
}

pub fn sunrise(base: [f32; 4], rain: f32, thunder: f32) -> [f32; 4] {
    let color = multiply_layer(
        base,
        rain - thunder,
        [127.0 / 255.0, 127.0 / 255.0, 153.0 / 255.0, 1.0],
    );
    multiply_layer(
        color,
        thunder,
        [63.0 / 255.0, 63.0 / 255.0, 76.0 / 255.0, 1.0],
    )
}

pub fn stars(mut brightness: f32, rain: f32, thunder: f32) -> f32 {
    let rain_only = rain - thunder;
    if rain_only > 0.0 {
        brightness += rain_only * (0.0 - brightness);
    }
    if thunder > 0.0 {
        brightness += thunder * (0.0 - brightness);
    }
    brightness.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clear_weather_preserves_colors_and_full_weather_desaturates_sky() {
        let color = [0.8, 0.3, 0.05];
        let sunrise_color = [0.8, 0.3, 0.05, 0.4];
        assert_eq!(sky(color, 0.0, 0.0), color);
        assert_eq!(fog(color, 0.0, 0.0), color);
        assert_eq!(sunrise(sunrise_color, 0.0, 0.0), sunrise_color);
        let rainy = sky(color, 1.0, 0.0);
        assert!(rainy[0] < color[0]);
        assert!(rainy[0] - rainy[2] < color[0] - color[2]);
        assert_eq!(sunrise(sunrise_color, 1.0, 1.0)[3], sunrise_color[3]);
    }
}
