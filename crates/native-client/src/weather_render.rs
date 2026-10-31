//! Precipitation columns from an immutable world snapshot.

use std::io::Cursor;

use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{DVec3, IVec3};
use image::{ImageFormat, RgbaImage};
use native_core::weather_columns;
use native_persistence::heightmap::SpawnHeightmapKind;
use native_resources::{biome_climate::Precipitation, packs::ResourceProvider};
use native_session::WorldSnapshot;
use native_world::loaded::ChunkPos;

pub struct WeatherTextures {
    pub rain: RgbaImage,
    pub snow: RgbaImage,
}

impl WeatherTextures {
    pub fn load(source: &mut impl ResourceProvider) -> Result<Self> {
        fn read(source: &mut impl ResourceProvider, name: &str) -> Result<RgbaImage> {
            let path = format!("assets/minecraft/textures/environment/{name}.png");
            let bytes = source
                .read_asset(&path)?
                .with_context(|| format!("missing {path}"))?;
            let mut reader = image::ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4096);
            limits.max_image_height = Some(4096);
            limits.max_alloc = Some(128 * 1024 * 1024);
            reader.limits(limits);
            let image = reader
                .decode()
                .with_context(|| format!("invalid {path}"))?
                .to_rgba8();
            ensure!(
                image.width() > 0 && image.height() > 0,
                "empty weather texture"
            );
            Ok(image)
        }
        Ok(Self {
            rain: read(source, "rain")?,
            snow: read(source, "snow")?,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct WeatherVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub opacity: f32,
    pub light: u32,
}

#[derive(Default)]
pub struct WeatherMesh {
    pub rain: Vec<WeatherVertex>,
    pub snow: Vec<WeatherVertex>,
}

/// Missing heightmaps or biome definitions leave the affected columns empty.
pub fn build_into(
    mesh: &mut WeatherMesh,
    snapshot: &WorldSnapshot,
    scene_origin: IVec3,
    camera: DVec3,
    partial: f32,
    radius: i32,
) {
    mesh.rain.clear();
    mesh.snow.clear();
    let Some(intensity) = snapshot
        .weather
        .map(|weather| weather.rain_at(partial))
        .filter(|&v| v > 0.0)
    else {
        return;
    };
    let Some(terrain) = &snapshot.weather_terrain else {
        return;
    };
    let Some(world) = snapshot.world.loaded() else {
        return;
    };
    if !(3..=10).contains(&radius) || world.biome_zoom_seed().is_none() || !camera.is_finite() {
        return;
    }
    let floor = camera.floor();
    if floor.min_element() < f64::from(i32::MIN + 16)
        || floor.max_element() > f64::from(i32::MAX - 16)
    {
        return;
    }
    let eye = floor.as_ivec3();
    let count = ((radius * 2 + 1) * (radius * 2 + 1) * 6) as usize;
    mesh.rain.reserve(count);
    for z in eye.z - radius..=eye.z + radius {
        for x in eye.x - radius..=eye.x + radius {
            let chunk_pos = ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            };
            let Some(chunk) = world.chunks.get(&chunk_pos) else {
                continue;
            };
            if chunk.status.as_deref() != Some("minecraft:full") {
                continue;
            }
            let Some(height) = terrain
                .heightmaps
                .get(&chunk_pos)
                .and_then(|map| map.highest(SpawnHeightmapKind::MotionBlocking, x, z))
                .and_then(|height| height.checked_add(1))
            else {
                continue;
            };
            let Some((bottom, top)) = weather_columns::vertical_range(eye.y, height, radius) else {
                continue;
            };
            let Some(biome) = world
                .biome_at_block_in_bounds(x, eye.y, z, terrain.min_y, terrain.max_y)
                .and_then(|id| terrain.climates.get(id))
            else {
                continue;
            };
            let kind = if !biome.has_precipitation {
                Precipitation::None
            } else if let Some(sea) = terrain.sea_level {
                biome.precipitation_at(x, eye.y, z, sea)
            } else {
                continue;
            };
            if kind == Precipitation::None {
                continue;
            }
            // The viewer already uses these visual fallbacks when a saved light
            // layer is omitted; simulation never consumes them.
            let light_y = eye.y.max(height);
            let light = [
                world.light(x, light_y, z, false).unwrap_or(0),
                world.light(x, light_y, z, true).unwrap_or(15),
            ];
            let animation = match kind {
                Precipitation::Rain => {
                    weather_columns::rain(x, z, snapshot.game_time, partial, light)
                }
                Precipitation::Snow => {
                    weather_columns::snow(x, z, snapshot.game_time, partial, light)
                }
                Precipitation::None => unreachable!(),
            };
            let target = if kind == Precipitation::Rain {
                &mut mesh.rain
            } else {
                &mut mesh.snow
            };
            append_column(
                target,
                x,
                z,
                bottom,
                top,
                eye,
                scene_origin,
                camera,
                radius,
                intensity,
                kind,
                animation,
            );
        }
    }
}

#[cfg(test)]
fn build(
    snapshot: &WorldSnapshot,
    scene_origin: IVec3,
    camera: DVec3,
    partial: f32,
    radius: i32,
) -> WeatherMesh {
    let mut mesh = WeatherMesh::default();
    build_into(&mut mesh, snapshot, scene_origin, camera, partial, radius);
    mesh
}

#[allow(clippy::too_many_arguments)]
fn append_column(
    target: &mut Vec<WeatherVertex>,
    x: i32,
    z: i32,
    bottom: i32,
    top: i32,
    eye: IVec3,
    origin: IVec3,
    camera: DVec3,
    radius: i32,
    intensity: f32,
    kind: Precipitation,
    animation: weather_columns::ColumnAnimation,
) {
    let dx = (x - eye.x) as f32;
    let dz = (z - eye.z) as f32;
    let length = (dx * dx + dz * dz).sqrt();
    if length == 0.0 {
        return;
    } // Java's center column contains NaN vertices.
    let half_x = -dz / length / 2.0;
    let half_z = dx / length / 2.0;
    let px = (f64::from(x) + 0.5 - camera.x) as f32;
    let pz = (f64::from(z) + 0.5 - camera.z) as f32;
    let fraction = ((px * px + pz * pz) / (radius * radius) as f32).min(1.0);
    let base = if kind == Precipitation::Rain {
        1.0
    } else {
        0.8
    };
    let alpha = ((base + fraction * (0.5 - base)) * intensity).clamp(0.0, 1.0);
    let alpha = (alpha * 255.0) as u8 as f32 / 255.0;
    let cx = (f64::from(x) + 0.5 - f64::from(origin.x)) as f32;
    let cz = (f64::from(z) + 0.5 - f64::from(origin.z)) as f32;
    let y0 = (f64::from(bottom) - f64::from(origin.y)) as f32;
    let y1 = (f64::from(top) - f64::from(origin.y)) as f32;
    let u0 = animation.u;
    let u1 = animation.u + 1.0;
    let v0 = bottom as f32 * 0.25 + animation.v;
    let v1 = top as f32 * 0.25 + animation.v;
    let light = u32::from(animation.sky_light) * 16 + u32::from(animation.block_light);
    let vertex = |position, uv| WeatherVertex {
        position,
        uv,
        opacity: alpha,
        light,
    };
    let a = vertex([cx - half_x, y1, cz - half_z], [u0, v0]);
    let b = vertex([cx + half_x, y1, cz + half_z], [u1, v0]);
    let c = vertex([cx + half_x, y0, cz + half_z], [u1, v1]);
    let d = vertex([cx - half_x, y0, cz - half_z], [u0, v1]);
    target.extend_from_slice(&[a, b, c, a, c, d]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_persistence::{
        heightmap::{BlockHeightFlags, HeightBounds, SpawnHeightmaps},
        nbt::{NamedTag, Tag},
    };
    use native_resources::biome_climate::{BiomeClimate, TemperatureModifier};
    use native_session::{
        GameSession, LocalSession, SessionWorld,
        weather::{WeatherTerrain, WeatherVisual},
    };
    use native_world::loaded::{BIOME_COUNT, Chunk, Section, World};
    use std::{collections::BTreeMap, sync::Arc};

    fn snapshot(temperature: f32, roof: i32) -> WorldSnapshot {
        let mut world = World::new("minecraft:overworld");
        world.set_biome_world_seed(Some(7));
        let biome = world.intern_biome("minecraft:plains".into());
        let pos = ChunkPos { x: 0, z: 0 };
        world.chunks.insert(
            pos,
            Arc::new(Chunk {
                position: pos,
                status: Some("minecraft:full".into()),
                block_light_correct: true,
                sections: (0..=1)
                    .map(|y| {
                        (
                            y,
                            Arc::new(Section {
                                blocks: None,
                                biomes: Some(Box::new([biome; BIOME_COUNT])),
                                block_light: None,
                                sky_light: None,
                            }),
                        )
                    })
                    .collect(),
            }),
        );
        let doc = NamedTag {
            name: "".into(),
            value: Tag::Compound(vec![]),
        };
        let mut maps = SpawnHeightmaps::read(
            &doc,
            HeightBounds {
                min_y: 0,
                max_y: 32,
            },
        )
        .unwrap();
        maps.prime_missing(|_, y, _| {
            Ok(BlockHeightFlags {
                is_air: y != roof,
                blocks_motion: y == roof,
                blocks_motion_no_leaves: y == roof,
                has_fluid: false,
            })
        })
        .unwrap();
        let mut snapshot = (*LocalSession::demo().snapshot()).clone();
        snapshot.world = SessionWorld::Loaded(Arc::new(world));
        snapshot.weather = Some(WeatherVisual {
            previous_rain: 1.0,
            rain: 1.0,
            ..Default::default()
        });
        snapshot.weather_terrain = Some(WeatherTerrain {
            heightmaps: Arc::new(BTreeMap::from([(pos, Arc::new(maps))])),
            climates: Arc::new(BTreeMap::from([(
                "minecraft:plains".into(),
                BiomeClimate {
                    has_precipitation: true,
                    temperature,
                    modifier: TemperatureModifier::None,
                },
            )])),
            sea_level: Some(63),
            min_y: 0,
            max_y: 32,
        });
        snapshot
    }

    #[test]
    fn columns_follow_height_biome_and_weather_without_drawing_missing_chunks() {
        let camera = DVec3::new(8.5, 10.0, 8.5);
        let rain = build(&snapshot(0.8, 0), IVec3::ZERO, camera, 0.5, 3);
        assert_eq!(rain.rain.len(), 48 * 6);
        assert!(rain.snow.is_empty());
        let high = build(
            &snapshot(0.8, 0),
            IVec3::ZERO,
            DVec3::new(8.5, 35.0, 8.5),
            0.5,
            3,
        );
        assert_eq!(
            high.rain.len(),
            48 * 6,
            "biomes clamp to the dimension above its build height"
        );
        assert!(rain.rain.iter().all(|v| (0.0..=1.0).contains(&v.opacity)
            && v.position[1] >= 7.0
            && v.position[1] <= 13.0));
        let snow = build(&snapshot(0.0, 0), IVec3::ZERO, camera, 0.5, 3);
        assert_eq!(snow.snow.len(), 48 * 6);
        assert!(snow.rain.is_empty());
        let covered = build(&snapshot(0.8, 13), IVec3::ZERO, camera, 0.5, 3);
        assert!(covered.rain.is_empty());
        let mut clear = snapshot(0.8, 0);
        clear.weather = Some(WeatherVisual::default());
        assert!(build(&clear, IVec3::ZERO, camera, 0.5, 3).rain.is_empty());
        clear.weather = None;
        assert!(build(&clear, IVec3::ZERO, camera, 0.5, 3).rain.is_empty());
    }

    #[test]
    fn column_vertices_keep_relative_precision_at_world_edges() {
        let mut vertices = Vec::new();
        let origin = IVec3::new(29_999_984, 0, -30_000_000);
        let eye = origin + IVec3::new(8, 10, 8);
        append_column(
            &mut vertices,
            eye.x + 1,
            eye.z,
            7,
            13,
            eye,
            origin,
            eye.as_dvec3() + DVec3::new(0.5, 0.0, 0.5),
            3,
            1.0,
            Precipitation::Rain,
            weather_columns::ColumnAnimation {
                u: 0.0,
                v: 0.0,
                block_light: 0,
                sky_light: 15,
            },
        );
        assert_eq!(vertices.len(), 6);
        assert_eq!(vertices[0].position[0], 9.5);
        assert_eq!(vertices[0].position[2], 8.0);
        assert_eq!(vertices[0].light, 240);
    }

    #[test]
    #[ignore = "requires the installed Minecraft 26.3 client"]
    fn textures_use_the_verified_client_and_selected_resource_pack() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut source = native_resources::packs::ResourceStack::open(&game).unwrap();
        let original = WeatherTextures::load(&mut source).unwrap();
        let pack = tempfile::tempdir().unwrap();
        let dir = pack.path().join("assets/minecraft/textures/environment");
        std::fs::create_dir_all(&dir).unwrap();
        let replacement = RgbaImage::from_pixel(2, 2, image::Rgba([42, 88, 200, 127]));
        replacement.save(dir.join("rain.png")).unwrap();
        source.push_pack(pack.path()).unwrap();
        let overridden = WeatherTextures::load(&mut source).unwrap();
        assert_eq!(overridden.rain, replacement);
        assert_eq!(overridden.snow, original.snow);
    }
}
