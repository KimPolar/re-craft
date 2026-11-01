//! Vanilla banner special-model geometry and item component decoding.

use anyhow::{Context, Result, ensure};
use glam::{Quat, Vec3};
use image::{RgbaImage, imageops::FilterType};
use native_persistence::nbt::Tag;
use native_resources::{
    geometry::Vertex,
    packs::{ResourceId, ResourceProvider},
};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Pattern {
    Registry(String),
    Direct(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Layer {
    pub pattern: Pattern,
    pub color: u32,
}

pub fn color(name: &str) -> Option<u32> {
    Some(match name {
        "white" => 0xf9fffe,
        "orange" => 0xf9801d,
        "magenta" => 0xc74ebd,
        "light_blue" => 0x3ab3da,
        "yellow" => 0xfed83d,
        "lime" => 0x80c71f,
        "pink" => 0xf38baa,
        "gray" => 0x474f52,
        "light_gray" => 0x9d9d97,
        "cyan" => 0x169c9c,
        "purple" => 0x8932b8,
        "blue" => 0x3c44aa,
        "brown" => 0x835432,
        "green" => 0x5e7c16,
        "red" => 0xb02e26,
        "black" => 0x1d1d21,
        _ => return None,
    })
}

/// Composite ordered, tinted pattern sprites into the flag texture.
pub fn overlay(base: &mut RgbaImage, layer: &RgbaImage, tint: u32) -> Result<()> {
    ensure!(
        base.width() == base.height() && layer.width() == layer.height() && layer.width() > 0,
        "invalid banner texture dimensions"
    );
    if layer.width() > base.width() {
        *base = image::imageops::resize(base, layer.width(), layer.height(), FilterType::Nearest);
    }
    let side = base.width();
    let channels = [
        ((tint >> 16) & 255) as u64,
        ((tint >> 8) & 255) as u64,
        (tint & 255) as u64,
    ];
    for y in 0..side {
        for x in 0..side {
            let src = layer
                .get_pixel(x * layer.width() / side, y * layer.height() / side)
                .0;
            let sa = u64::from(src[3]);
            if sa == 0 {
                continue;
            }
            let pixel = base.get_pixel_mut(x, y);
            let da = u64::from(pixel[3]);
            let alpha = sa * 255 + da * (255 - sa);
            if alpha == 0 {
                continue;
            }
            for c in 0..3 {
                let tinted = (u64::from(src[c]) * channels[c] + 127) / 255;
                pixel[c] = ((tinted * sa * 255 + u64::from(pixel[c]) * da * (255 - sa) + alpha / 2)
                    / alpha) as u8;
            }
            pixel[3] = ((alpha + 127) / 255) as u8;
        }
    }
    Ok(())
}

fn string(value: &Tag) -> Option<String> {
    match value {
        Tag::String(s) => String::from_utf16(&s.0).ok(),
        _ => None,
    }
}

pub fn patterns(components: &Tag) -> Option<Vec<Layer>> {
    if components.get("!minecraft:banner_patterns").is_some() {
        return Some(Vec::new());
    }
    let Some(value) = components.get("minecraft:banner_patterns") else {
        return Some(Vec::new());
    };
    let Tag::List { values, .. } = value else {
        return None;
    };
    values
        .iter()
        .take(16)
        .map(|layer| {
            let pattern = match layer.get("pattern")? {
                Tag::String(value) => Pattern::Registry(String::from_utf16(&value.0).ok()?),
                value @ Tag::Compound(_) => Pattern::Direct(string(value.get("asset_id")?)?),
                _ => return None,
            };
            let color = color(&string(layer.get("color")?)?)?;
            Some(Layer { pattern, color })
        })
        .collect()
}

pub fn asset_id(provider: &mut impl ResourceProvider, pattern: &Pattern) -> Result<ResourceId> {
    let id = match pattern {
        Pattern::Direct(id) => id.clone(),
        Pattern::Registry(id) => {
            let id = ResourceId::parse(id)?;
            let path = format!("data/{}/banner_pattern/{}.json", id.namespace, id.path);
            let bytes = provider
                .read_data(&path)?
                .with_context(|| format!("missing banner pattern {path}"))?;
            let data: Value = serde_json::from_slice(&bytes)?;
            data.get("asset_id")
                .and_then(Value::as_str)
                .context("missing banner asset_id")?
                .to_owned()
        }
    };
    ResourceId::parse(&id)
}

#[derive(Clone, Copy)]
pub struct Quad {
    pub vertices: [Vertex; 4],
    pub normal: [f32; 3],
}

fn vector(value: Option<&Value>, default: [f32; 3]) -> Result<Vec3> {
    let Some(value) = value else {
        return Ok(Vec3::from_array(default));
    };
    let array = value
        .as_array()
        .context("invalid special transformation vector")?;
    ensure!(array.len() == 3, "invalid special transformation vector");
    let mut values = [0.; 3];
    for (out, v) in values.iter_mut().zip(array) {
        *out = v
            .as_f64()
            .context("invalid special transformation coordinate")? as f32;
        ensure!(out.is_finite(), "invalid special transformation coordinate");
    }
    Ok(Vec3::from_array(values))
}
fn quaternion(value: Option<&Value>) -> Result<Quat> {
    let Some(value) = value else {
        return Ok(Quat::IDENTITY);
    };
    let array = value.as_array().context("invalid special rotation")?;
    ensure!(array.len() == 4, "invalid special rotation");
    let mut v = [0.; 4];
    for (out, n) in v.iter_mut().zip(array) {
        *out = n.as_f64().context("invalid special rotation")? as f32;
        ensure!(out.is_finite(), "invalid special rotation");
    }
    let q = Quat::from_xyzw(v[0], v[1], v[2], v[3]);
    ensure!(q.length_squared() > 0., "zero special rotation");
    Ok(q.normalize())
}

struct Transform {
    translation: Vec3,
    left: Quat,
    scale: Vec3,
    right: Quat,
}
impl Transform {
    fn parse(value: Option<&Value>) -> Result<Self> {
        if let Some(value) = value {
            ensure!(
                value.is_object(),
                "special transformation must be an object"
            );
        }
        let get = |key| value.and_then(|v| v.get(key));
        let scale = vector(get("scale"), [1.; 3])?;
        ensure!(
            scale.to_array().iter().all(|v| v.abs() > 1e-8),
            "zero special scale"
        );
        Ok(Self {
            translation: vector(get("translation"), [0.; 3])?,
            left: quaternion(get("left_rotation"))?,
            scale,
            right: quaternion(get("right_rotation"))?,
        })
    }
    fn position(&self, p: Vec3) -> Vec3 {
        self.translation + self.left * (self.scale * (self.right * p))
    }
    fn normal(&self, n: Vec3) -> Vec3 {
        (self.left * ((self.right * n) / self.scale)).normalize_or_zero()
    }
}

fn cube(
    from: Vec3,
    size: Vec3,
    uv: [f32; 2],
    pivot: Vec3,
    rotation: Quat,
    transform: &Transform,
) -> Vec<Quad> {
    let [x0, y0, z0] = from.to_array();
    let [x1, y1, z1] = (from + size).to_array();
    let [w, h, d] = size.to_array();
    let [u, v] = uv;
    let faces: [([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]); 6] = [
        (
            [[x1, y0, z1], [x0, y0, z1], [x0, y0, z0], [x1, y0, z0]],
            [
                [u + d + w, v],
                [u + d, v],
                [u + d, v + d],
                [u + d + w, v + d],
            ],
            [0., -1., 0.],
        ),
        (
            [[x1, y1, z0], [x0, y1, z0], [x0, y1, z1], [x1, y1, z1]],
            [
                [u + d + 2. * w, v + d],
                [u + d + w, v + d],
                [u + d + w, v],
                [u + d + 2. * w, v],
            ],
            [0., 1., 0.],
        ),
        (
            [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
            [
                [u + d, v + d],
                [u, v + d],
                [u, v + d + h],
                [u + d, v + d + h],
            ],
            [-1., 0., 0.],
        ),
        (
            [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
            [
                [u + d + w, v + d],
                [u + d, v + d],
                [u + d, v + d + h],
                [u + d + w, v + d + h],
            ],
            [0., 0., -1.],
        ),
        (
            [[x1, y0, z1], [x1, y0, z0], [x1, y1, z0], [x1, y1, z1]],
            [
                [u + 2. * d + w, v + d],
                [u + d + w, v + d],
                [u + d + w, v + d + h],
                [u + 2. * d + w, v + d + h],
            ],
            [1., 0., 0.],
        ),
        (
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            [
                [u + 2. * d + 2. * w, v + d],
                [u + 2. * d + w, v + d],
                [u + 2. * d + w, v + d + h],
                [u + 2. * d + 2. * w, v + d + h],
            ],
            [0., 0., 1.],
        ),
    ];
    faces
        .into_iter()
        .map(|(positions, uvs, normal)| Quad {
            vertices: std::array::from_fn(|i| Vertex {
                position: transform
                    .position((pivot + rotation * Vec3::from_array(positions[i])) / 16.)
                    .to_array(),
                uv: [uvs[i][0] / 64., uvs[i][1] / 64.],
            }),
            normal: transform
                .normal(rotation * Vec3::from_array(normal))
                .to_array(),
        })
        .collect()
}

pub fn geometry(transformation: Option<&Value>) -> Result<(Vec<Quad>, Vec<Quad>)> {
    let transform = Transform::parse(transformation)?;
    let mut body = cube(
        Vec3::new(-10., -44., -1.),
        Vec3::new(20., 2., 2.),
        [0., 42.],
        Vec3::ZERO,
        Quat::IDENTITY,
        &transform,
    );
    body.extend(cube(
        Vec3::new(-1., -42., -1.),
        Vec3::new(2., 42., 2.),
        [44., 0.],
        Vec3::ZERO,
        Quat::IDENTITY,
        &transform,
    ));
    let flag = cube(
        Vec3::new(-10., 0., -2.),
        Vec3::new(20., 40., 1.),
        [0., 0.],
        Vec3::new(0., -44., 0.),
        Quat::from_rotation_x(-0.0025 * std::f32::consts::PI),
        &transform,
    );
    Ok((body, flag))
}

/// Java's block entity renderer uses a different model for wall banners and
/// computes the cloth angle from the block position and world time.
pub fn world_geometry(
    state: &native_world::loaded::BlockState,
    phase: f32,
) -> Option<(Vec<Quad>, Vec<Quad>)> {
    native_world::collision::banner_item_id(state)?;
    if !phase.is_finite() {
        return None;
    }
    let wall = state.name.ends_with("_wall_banner");
    let orientation = world_orientation(state)?;
    let transform = Transform {
        translation: Vec3::new(0.5, 0.0, 0.5),
        left: orientation,
        scale: Vec3::new(2.0 / 3.0, -2.0 / 3.0, -2.0 / 3.0),
        right: Quat::IDENTITY,
    };
    let (bar_y, bar_z, pivot_z) = if wall {
        (-20.5, 9.5, 10.5)
    } else {
        (-44.0, -1.0, 0.0)
    };
    let mut body = cube(
        Vec3::new(-10., bar_y, bar_z),
        Vec3::new(20., 2., 2.),
        [0., 42.],
        Vec3::ZERO,
        Quat::IDENTITY,
        &transform,
    );
    if !wall {
        body.extend(cube(
            Vec3::new(-1., -42., -1.),
            Vec3::new(2., 42., 2.),
            [44., 0.],
            Vec3::ZERO,
            Quat::IDENTITY,
            &transform,
        ));
    }
    let flag = cube(
        Vec3::new(-10., 0., -2.),
        Vec3::new(20., 40., 1.),
        [0., 0.],
        Vec3::new(0., bar_y, pivot_z),
        Quat::from_rotation_x(flag_angle(phase)),
        &transform,
    );
    Some((body, flag))
}

pub fn world_orientation(state: &native_world::loaded::BlockState) -> Option<Quat> {
    native_world::collision::banner_item_id(state)?;
    let degrees = if state.name.ends_with("_wall_banner") {
        match state.properties.get("facing")?.as_str() {
            "south" => 0.0,
            "west" => 90.0,
            "north" => 180.0,
            "east" => 270.0,
            _ => return None,
        }
    } else {
        state.properties.get("rotation")?.parse::<f32>().ok()? * 22.5
    };
    Some(Quat::from_rotation_y(
        -degrees * std::f32::consts::PI / 180.0,
    ))
}

pub fn flag_angle(phase: f32) -> f32 {
    // Mth.cos indexes a 65536-entry sine table rather than calling cos directly.
    let radians = std::f32::consts::TAU * phase;
    let index = ((f64::from(radians) * 10430.378350470453 + 16384.0) as i64) & 65535;
    let cosine = ((index as f64) * std::f64::consts::TAU / 65536.0).sin() as f32;
    (-0.0125 + 0.01 * cosine) * std::f32::consts::PI
}

pub fn world_phase(position: glam::IVec3, game_time: i64, partial_tick: f32) -> f32 {
    let offset = position
        .x
        .wrapping_mul(7)
        .wrapping_add(position.y.wrapping_mul(9))
        .wrapping_add(position.z.wrapping_mul(13));
    ((i64::from(offset).wrapping_add(game_time)).rem_euclid(100) as f32 + partial_tick) / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "windows")]
    use std::{path::PathBuf, process::Command};

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn world_geometry_matches_java_for_rotations_and_animation() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let metadata: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in metadata["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let base = temp.path().join("BannerGeometryOracle.java");
        let source = temp.path().join("WorldBannerGeometryOracle.java");
        let output = temp.path().join("world-banners.json");
        std::fs::write(
            &base,
            include_str!("../tests/java/BannerGeometryOracle.java"),
        )
        .unwrap();
        std::fs::write(
            &source,
            include_str!("../tests/java/WorldBannerGeometryOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&base)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("WorldBannerGeometryOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let rows: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 100);
        for row in rows.as_array().unwrap() {
            let index = row["index"].as_u64().unwrap() as usize;
            let state = native_world::loaded::BlockState {
                name: if index < 16 {
                    "minecraft:white_banner"
                } else {
                    "minecraft:white_wall_banner"
                }
                .into(),
                properties: if index < 16 {
                    [("rotation".into(), index.to_string())].into()
                } else {
                    [(
                        "facing".into(),
                        ["south", "west", "north", "east"][index - 16].into(),
                    )]
                    .into()
                },
            };
            let (body, flag) =
                world_geometry(&state, row["phase"].as_f64().unwrap() as f32).unwrap();
            for (name, quads) in [("body", body), ("flag", flag)] {
                let expected = row[name].as_array().unwrap();
                assert_eq!(expected.len(), quads.len() * 4, "{row}");
                for (quad, expected) in quads.iter().zip(expected.chunks_exact(4)) {
                    for (vertex, want) in quad.vertices.iter().zip(expected) {
                        for (actual, want) in vertex
                            .position
                            .iter()
                            .zip(want["position"].as_array().unwrap())
                        {
                            assert!(
                                (f64::from(*actual) - want.as_f64().unwrap()).abs() < 0.000002,
                                "{row}"
                            );
                        }
                        for (actual, want) in vertex.uv.iter().zip(want["uv"].as_array().unwrap()) {
                            assert!(
                                (f64::from(*actual) - want.as_f64().unwrap()).abs() < 0.000002,
                                "{row}"
                            );
                        }
                        for (actual, want) in
                            quad.normal.iter().zip(want["normal"].as_array().unwrap())
                        {
                            assert!(
                                (f64::from(*actual) - want.as_f64().unwrap()).abs() < 0.000002,
                                "{row}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn pattern_resolution_and_overlay_preserve_order_and_alpha() {
        struct Data;
        impl ResourceProvider for Data {
            fn read_asset(&mut self, _: &str) -> Result<Option<Vec<u8>>> {
                Ok(None)
            }
            fn read_data(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
                Ok(
                    (path == "data/example/banner_pattern/custom.json").then(|| {
                        br#"{"asset_id":"example:design","translation_key":"example.banner"}"#
                            .to_vec()
                    }),
                )
            }
        }
        let registry = Pattern::Registry("example:custom".into());
        let direct = Pattern::Direct("example:design".into());
        assert_eq!(
            asset_id(&mut Data, &registry).unwrap(),
            ResourceId::parse("example:design").unwrap()
        );
        assert_eq!(
            asset_id(&mut Data, &direct).unwrap(),
            ResourceId::parse("example:design").unwrap()
        );
        let mut base = RgbaImage::from_pixel(1, 1, image::Rgba([20, 40, 60, 255]));
        let layer = RgbaImage::from_pixel(2, 2, image::Rgba([200, 100, 50, 128]));
        overlay(&mut base, &layer, 0xff8000).unwrap();
        assert_eq!(base.dimensions(), (2, 2));
        assert_eq!(base.get_pixel(0, 0).0, [110, 45, 30, 255]);
        overlay(
            &mut base,
            &RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255])),
            0x123456,
        )
        .unwrap();
        assert_eq!(base.get_pixel(1, 1).0, [0x12, 0x34, 0x56, 255]);
    }

    #[test]
    fn banner_cache_uses_only_the_sixteen_rendered_layers() {
        use native_persistence::nbt::TagType;
        let layer = Tag::Compound(vec![
            ("pattern".into(), Tag::String("minecraft:cross".into())),
            ("color".into(), Tag::String("red".into())),
        ]);
        let components = |n| {
            Tag::Compound(vec![(
                "minecraft:banner_patterns".into(),
                Tag::List {
                    element_type: TagType::Compound,
                    values: vec![layer.clone(); n],
                },
            )])
        };
        assert_eq!(patterns(&components(16)), patterns(&components(20)));
        assert_eq!(patterns(&components(20)).unwrap().len(), 16);
    }

    #[test]
    #[cfg(target_os = "windows")]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn geometry_and_dye_colors_match_java() {
        use native_resources::packs::ResourceStack;
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut resources = ResourceStack::open(&game).unwrap();
        let definition: Value = serde_json::from_slice(
            &resources
                .read_asset("assets/minecraft/items/white_banner.json")
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for lib in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java=std::env::var_os("MINECRAFT_JAVA_HOME").or_else(||std::env::var_os("JAVA_HOME")).map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let source = temp.path().join("BannerGeometryOracle.java");
        let output = temp.path().join("geometry.json");
        std::fs::write(
            &source,
            include_str!("../tests/java/BannerGeometryOracle.java"),
        )
        .unwrap();
        for command in [
            Command::new(java.join("javac.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg(&source),
            Command::new(java.join("java.exe"))
                .arg("-cp")
                .arg(&cp)
                .arg("BannerGeometryOracle")
                .arg(&output),
        ] {
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let reference: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        let (body, flag) = geometry(definition["model"].get("transformation")).unwrap();
        for (name, quads) in [("body", body), ("flag", flag)] {
            let expected = reference[name].as_array().unwrap();
            assert_eq!(expected.len(), quads.len() * 4);
            for (quad, expected) in quads.iter().zip(expected.chunks_exact(4)) {
                for (vertex, expected) in quad.vertices.iter().zip(expected) {
                    for (actual, want) in vertex
                        .position
                        .iter()
                        .zip(expected["position"].as_array().unwrap())
                    {
                        assert!(
                            (f64::from(*actual) - want.as_f64().unwrap()).abs() < 0.000001,
                            "{name} {expected}"
                        );
                    }
                    for (actual, want) in vertex.uv.iter().zip(expected["uv"].as_array().unwrap()) {
                        assert!(
                            (f64::from(*actual) - want.as_f64().unwrap()).abs() < 0.000001,
                            "{name} {expected}"
                        );
                    }
                    for (actual, want) in quad
                        .normal
                        .iter()
                        .zip(expected["normal"].as_array().unwrap())
                    {
                        assert!(
                            (f64::from(*actual) - want.as_f64().unwrap()).abs() < 0.000001,
                            "{name} {expected}"
                        );
                    }
                }
            }
        }
        for (name, value) in reference["colors"].as_object().unwrap() {
            assert_eq!(
                color(name),
                Some(value.as_i64().unwrap() as u32 & 0xffffff),
                "{name}"
            );
        }
    }
}
