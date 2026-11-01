//! Animated particle textures, shared by all particles using the same atlas.

use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use native_resources::{
    packs::{ResourceId, ResourceProvider},
    sprites::SpriteAtlas,
};
use std::{collections::VecDeque, sync::Arc};

type AnimationKey = Vec<(u32, u32, u16)>;
const FRAME_CACHE_COUNT: usize = 8;
const FRAME_CACHE_BYTES: usize = 16 * 1024 * 1024;

struct CachedFrame {
    key: AnimationKey,
    image: Arc<RgbaImage>,
}

pub(crate) struct ParticleSprites {
    atlas: SpriteAtlas,
    coordinates: Vec<[f32; 4]>,
    current_key: AnimationKey,
    last_tick: u64,
    image: Arc<RgbaImage>,
    cache: VecDeque<CachedFrame>,
    cache_bytes: usize,
}

impl ParticleSprites {
    pub fn load(source: &mut impl ResourceProvider, name: &str) -> Result<Self> {
        let bytes = source
            .read_asset(&format!("assets/minecraft/particles/{name}.json"))?
            .with_context(|| format!("missing {name} particle definition"))?;
        let definition: serde_json::Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid {name} particle definition"))?;
        let textures = definition
            .get("textures")
            .and_then(serde_json::Value::as_array)
            .with_context(|| format!("missing {name} textures"))?;
        ensure!(
            !textures.is_empty() && textures.len() <= 256,
            "invalid {name} texture count"
        );
        let ids = textures
            .iter()
            .map(|value| {
                let id = ResourceId::parse(value.as_str().context("invalid particle texture ID")?)?;
                Ok(ResourceId {
                    namespace: id.namespace,
                    path: format!("particle/{}", id.path),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let atlas = SpriteAtlas::build(source, ids.iter().cloned(), 4096)?;
        let width = atlas.texture.width as f32;
        let height = atlas.texture.height as f32;
        let coordinates = ids
            .iter()
            .map(|id| {
                let rect = atlas
                    .rectangle(id)
                    .context("particle atlas sprite is unavailable")?;
                Ok([
                    rect.x as f32 / width,
                    rect.y as f32 / height,
                    (rect.x + rect.width) as f32 / width,
                    (rect.y + rect.height) as f32 / height,
                ])
            })
            .collect::<Result<Vec<_>>>()?;
        let image = Arc::new(
            RgbaImage::from_raw(
                atlas.texture.width,
                atlas.texture.height,
                atlas.texture.rgba.clone(),
            )
            .context("invalid particle atlas image")?,
        );
        let current_key = atlas.animation_key(0);
        let bytes = image.as_raw().len();
        let mut cache = VecDeque::new();
        if bytes <= FRAME_CACHE_BYTES {
            cache.push_back(CachedFrame {
                key: current_key.clone(),
                image: Arc::clone(&image),
            });
        }
        Ok(Self {
            atlas,
            coordinates,
            current_key,
            last_tick: 0,
            image,
            cache,
            cache_bytes: if bytes <= FRAME_CACHE_BYTES { bytes } else { 0 },
        })
    }

    pub fn len(&self) -> usize {
        self.coordinates.len()
    }

    pub fn get(&self, index: usize) -> Option<(&Arc<RgbaImage>, [f32; 4])> {
        self.coordinates
            .get(index)
            .copied()
            .map(|uv| (&self.image, uv))
    }

    pub fn advance(&mut self, tick: u64) {
        if tick == self.last_tick {
            return;
        }
        self.last_tick = tick;
        let key = self.atlas.animation_key(tick);
        if key == self.current_key {
            return;
        }
        if let Some(index) = self.cache.iter().position(|frame| frame.key == key) {
            let frame = self.cache.remove(index).expect("cached frame exists");
            self.image = Arc::clone(&frame.image);
            self.current_key = key;
            self.cache.push_back(frame);
            return;
        }
        self.atlas.update(tick);
        let image = Arc::new(
            RgbaImage::from_raw(
                self.atlas.texture.width,
                self.atlas.texture.height,
                self.atlas.texture.rgba.clone(),
            )
            .expect("validated atlas dimensions"),
        );
        self.current_key = key.clone();
        self.image = Arc::clone(&image);
        let bytes = image.as_raw().len();
        while !self.cache.is_empty()
            && (self.cache.len() >= FRAME_CACHE_COUNT
                || self.cache_bytes.saturating_add(bytes) > FRAME_CACHE_BYTES)
        {
            let old = self.cache.pop_front().expect("cache is nonempty");
            self.cache_bytes -= old.image.as_raw().len();
        }
        if bytes <= FRAME_CACHE_BYTES {
            self.cache_bytes += bytes;
            self.cache.push_back(CachedFrame { key, image });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba};
    use std::{collections::BTreeMap, io::Cursor};

    #[derive(Default)]
    struct Memory(BTreeMap<String, Vec<u8>>);
    impl ResourceProvider for Memory {
        fn read_asset(&mut self, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(self.0.get(path).cloned())
        }
    }
    fn png(image: RgbaImage) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }
    fn pixel(sprites: &ParticleSprites, index: usize) -> [u8; 4] {
        let (image, uv) = sprites.get(index).unwrap();
        image
            .get_pixel(
                (uv[0] * image.width() as f32) as u32,
                (uv[1] * image.height() as f32) as u32,
            )
            .0
    }

    #[test]
    fn animated_pack_frames_keep_sprite_order_and_reuse_repeating_atlas_images() {
        let mut source = Memory::default();
        source.0.insert(
            "assets/minecraft/particles/bubble.json".into(),
            br#"{"textures":["minecraft:custom","minecraft:custom"]}"#.to_vec(),
        );
        let mut sheet = RgbaImage::from_pixel(2, 6, Rgba([255, 0, 0, 255]));
        for y in 2..4 {
            for x in 0..2 {
                sheet.put_pixel(x, y, Rgba([0, 255, 0, 255]));
            }
        }
        for y in 4..6 {
            for x in 0..2 {
                sheet.put_pixel(x, y, Rgba([0, 0, 255, 255]));
            }
        }
        source.0.insert(
            "assets/minecraft/textures/particle/custom.png".into(),
            png(sheet),
        );
        source.0.insert(
            "assets/minecraft/textures/particle/custom.png.mcmeta".into(),
            br#"{"animation":{"frametime":2,"frames":[2,{"index":0,"time":3},1]}}"#.to_vec(),
        );
        let mut sprites = ParticleSprites::load(&mut source, "bubble").unwrap();
        assert_eq!(sprites.len(), 2);
        assert_eq!(sprites.get(0).unwrap().1, sprites.get(1).unwrap().1);
        assert_eq!(pixel(&sprites, 0), [0, 0, 255, 255]);
        let first = Arc::clone(sprites.get(0).unwrap().0);
        sprites.advance(1);
        assert!(Arc::ptr_eq(&first, sprites.get(0).unwrap().0));
        sprites.advance(2);
        assert_eq!(pixel(&sprites, 0), [255, 0, 0, 255]);
        let red = Arc::clone(sprites.get(0).unwrap().0);
        sprites.advance(5);
        assert_eq!(pixel(&sprites, 1), [0, 255, 0, 255]);
        sprites.advance(7);
        assert_eq!(pixel(&sprites, 0), [0, 0, 255, 255]);
        assert!(Arc::ptr_eq(&first, sprites.get(0).unwrap().0));
        sprites.advance(9);
        assert!(Arc::ptr_eq(&red, sprites.get(0).unwrap().0));
        assert_eq!(sprites.cache.len(), 3);
    }

    #[test]
    fn interpolated_particle_frames_follow_metadata_and_bound_the_cache() {
        let mut source = Memory::default();
        source.0.insert(
            "assets/minecraft/particles/large_smoke.json".into(),
            br#"{"textures":["minecraft:smoke"]}"#.to_vec(),
        );
        let mut sheet = RgbaImage::from_pixel(2, 4, Rgba([0, 0, 0, 0]));
        for y in 2..4 {
            for x in 0..2 {
                sheet.put_pixel(x, y, Rgba([200, 100, 40, 200]));
            }
        }
        source.0.insert(
            "assets/minecraft/textures/particle/smoke.png".into(),
            png(sheet),
        );
        source.0.insert(
            "assets/minecraft/textures/particle/smoke.png.mcmeta".into(),
            br#"{"animation":{"frametime":16,"interpolate":true}}"#.to_vec(),
        );
        let mut sprites = ParticleSprites::load(&mut source, "large_smoke").unwrap();
        sprites.advance(4);
        assert_eq!(pixel(&sprites, 0), [50, 25, 10, 50]);
        sprites.advance(8);
        assert_eq!(pixel(&sprites, 0), [100, 50, 20, 100]);
        for tick in 9..24 {
            sprites.advance(tick);
        }
        assert!(sprites.cache.len() <= FRAME_CACHE_COUNT);
        assert!(sprites.cache_bytes <= FRAME_CACHE_BYTES);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn animation_frame_timing_matches_java_sprite_state() {
        use std::{path::PathBuf, process::Command};
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let version = game.join("versions/26.3");
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mut paths = vec![version.join("26.3.jar"), temp.path().to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        let cp = std::env::join_paths(paths).unwrap();
        let java = std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME")).map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon")).join("bin");
        let path = temp.path().join("SpriteAnimationOracle.java");
        let result = temp.path().join("animation.json");
        std::fs::write(
            &path,
            include_str!("../tests/java/SpriteAnimationOracle.java"),
        )
        .unwrap();
        let run = |command: &mut Command| {
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(Command::new(java.join("javac.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg(&path));
        run(Command::new(java.join("java.exe"))
            .arg("-cp")
            .arg(&cp)
            .arg("SpriteAnimationOracle")
            .arg(&result));
        let rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(result).unwrap()).unwrap();
        assert_eq!(rows.len(), 96);
        for interpolate in [false, true] {
            let mut source = Memory::default();
            source.0.insert(
                "assets/minecraft/particles/bubble.json".into(),
                br#"{"textures":["minecraft:test"]}"#.to_vec(),
            );
            source.0.insert(
                "assets/minecraft/textures/particle/test.png".into(),
                png(RgbaImage::from_pixel(2, 6, Rgba([20, 40, 60, 255]))),
            );
            let metadata = serde_json::json!({"animation":{"interpolate":interpolate,"frames":[
                {"index":2,"time":2},{"index":0,"time":3},{"index":1,"time":2},{"index":1,"time":1}
            ]}});
            source.0.insert(
                "assets/minecraft/textures/particle/test.png.mcmeta".into(),
                serde_json::to_vec(&metadata).unwrap(),
            );
            let mut sprites = ParticleSprites::load(&mut source, "bubble").unwrap();
            for row in rows
                .iter()
                .filter(|row| row["interpolate"].as_bool().unwrap() == interpolate)
            {
                sprites.advance(row["tick"].as_u64().unwrap());
                assert_eq!(
                    sprites.current_key,
                    vec![(
                        row["current"].as_u64().unwrap() as u32,
                        row["next"].as_u64().unwrap() as u32,
                        row["progress"].as_u64().unwrap() as u16
                    )],
                    "{row}"
                );
            }
        }
    }
}
