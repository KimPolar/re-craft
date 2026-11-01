use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use image::{GenericImage, RgbaImage};
use serde::Deserialize;
use sha1::{Digest, Sha1};
use zip::ZipArchive;

pub mod asset_index;
pub mod biome_climate;
pub mod biome_source;
pub mod biomes;
pub mod block_registry;
pub mod blockstates;
pub mod carver;
pub mod datapacks;
pub mod density;
pub mod enchantments;
pub mod entity_collision;
pub mod fluid_environment;
pub mod fluid_tags;
pub mod geometry;
pub mod heightmap_tags;
pub mod item_registry;
pub mod loot;
pub mod mining_tags;
pub mod models;
pub mod noise;
pub mod noise_router;
pub mod noise_stage;
pub mod packs;
pub mod recipes;
pub mod respawn_environment;
pub mod respawn_tags;
pub mod sounds;
pub mod sprites;
pub mod structure_biomes;
pub mod structure_placement;
pub mod surface_biomes;
pub mod surface_pass;
pub mod surface_rules;
pub mod transforms;
pub mod vanilla_atlas;
pub mod vanilla_stitcher;

pub const TARGET_VERSION: &str = "26.3";
pub const DEMO_PLANKS_TILE_START: u32 = 5;
pub const DEMO_CRAFTING_TABLE_TOP: u32 =
    DEMO_PLANKS_TILE_START + native_core::wood::Planks::ALL.len() as u32;
pub const DEMO_CRAFTING_TABLE_FRONT: u32 = DEMO_CRAFTING_TABLE_TOP + 1;
pub const DEMO_CRAFTING_TABLE_SIDE: u32 = DEMO_CRAFTING_TABLE_TOP + 2;
pub const DEMO_COBBLESTONE: u32 = DEMO_CRAFTING_TABLE_SIDE + 1;
pub const DEMO_WOOD_TILE_START: u32 = DEMO_COBBLESTONE + 1;
pub const DEMO_SOIL_TILE_START: u32 = DEMO_WOOD_TILE_START + native_core::wood::Wood::TEXTURE_COUNT;
pub const DEMO_SOIL_TEXTURES: [&str; 13] = [
    "coarse_dirt.png",
    "rooted_dirt.png",
    "podzol_top.png",
    "podzol_side.png",
    "mycelium_top.png",
    "mycelium_side.png",
    "dirt_path_top.png",
    "dirt_path_side.png",
    "farmland.png",
    "farmland_moist.png",
    "sand.png",
    "red_sand.png",
    "gravel.png",
];
pub const DEMO_ORE_TILE_START: u32 = DEMO_SOIL_TILE_START + DEMO_SOIL_TEXTURES.len() as u32;
pub const DEMO_ATLAS_TILES: u32 = DEMO_ORE_TILE_START + native_core::ore::Ore::ALL.len() as u32;
pub const DEMO_ATLAS_COLUMNS: u32 = 8;
pub const DEMO_ATLAS_ROWS: u32 = DEMO_ATLAS_TILES.div_ceil(DEMO_ATLAS_COLUMNS);
const TEXTURE_ROOT: &str = "assets/minecraft/textures/block/";

#[derive(Clone, Debug)]
pub struct TextureAtlas {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Deserialize)]
struct Manifest {
    id: String,
    downloads: Downloads,
}

#[derive(Deserialize)]
struct Downloads {
    client: ClientDownload,
}

#[derive(Deserialize)]
struct ClientDownload {
    sha1: String,
}

#[derive(Deserialize)]
struct JarVersion {
    id: String,
    world_version: u32,
}

pub fn default_minecraft_dir() -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA")
        .context("APPDATA is unavailable; choose the Minecraft game folder")?;
    Ok(PathBuf::from(appdata).join(".minecraft"))
}

/// Opens the verified local client without extracting or modifying its files.
pub(crate) fn open_verified_client(minecraft_dir: &Path) -> Result<ZipArchive<File>> {
    let version_dir = minecraft_dir.join("versions").join(TARGET_VERSION);
    let metadata_path = version_dir.join(format!("{TARGET_VERSION}.json"));
    let jar_path = version_dir.join(format!("{TARGET_VERSION}.jar"));
    let metadata = std::fs::read(&metadata_path).with_context(|| {
        format!(
            "cannot read {}; install Java Edition 26.3 or choose its game folder",
            metadata_path.display()
        )
    })?;
    let manifest: Manifest = serde_json::from_slice(&metadata)
        .with_context(|| format!("invalid version metadata: {}", metadata_path.display()))?;
    if manifest.id != TARGET_VERSION {
        bail!(
            "expected Minecraft {TARGET_VERSION}, but metadata identifies {}",
            manifest.id
        );
    }

    let mut jar_file = File::open(&jar_path)
        .with_context(|| format!("cannot open Minecraft client JAR: {}", jar_path.display()))?;
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = jar_file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual_hash = format!("{:x}", hasher.finalize());
    if !actual_hash.eq_ignore_ascii_case(&manifest.downloads.client.sha1) {
        bail!(
            "Minecraft client JAR checksum differs from its 26.3 version metadata: {}",
            jar_path.display()
        );
    }

    let file = File::open(&jar_path)?;
    let mut zip =
        ZipArchive::new(file).context("Minecraft client JAR is not a valid ZIP archive")?;
    let jar_version: JarVersion = {
        let version = zip
            .by_name("version.json")
            .context("Minecraft client JAR is missing version.json")?;
        serde_json::from_reader(version)
            .context("invalid version.json inside Minecraft client JAR")?
    };
    if jar_version.id != TARGET_VERSION || jar_version.world_version != 5023 {
        bail!("client JAR is not Minecraft 26.3 (world data version 5023)");
    }

    Ok(zip)
}

/// Verifies the locally installed client, then reads its original textures without extracting them.
pub fn load_atlas(minecraft_dir: &Path) -> Result<TextureAtlas> {
    load_atlas_with_packs(minecraft_dir, &[])
}

/// Loads the demo textures using the same pack precedence as model lookup.
pub fn load_atlas_with_packs(minecraft_dir: &Path, pack_paths: &[PathBuf]) -> Result<TextureAtlas> {
    use packs::ResourceProvider;
    let mut sources = packs::ResourceStack::open(minecraft_dir)?;
    for path in pack_paths {
        sources.push_pack(path)?;
    }
    let mut tiles = Vec::new();
    let basic = [
        "grass_block_top.png",
        "grass_block_side.png",
        "grass_block_side_overlay.png",
        "dirt.png",
        "stone.png",
        "stone_bricks.png",
    ];
    let names = basic
        .into_iter()
        .map(str::to_owned)
        .chain(
            native_core::wood::Planks::ALL
                .into_iter()
                .map(|planks| format!("{}.png", planks.id().strip_prefix("minecraft:").unwrap()))
                .chain(
                    [
                        "crafting_table_top.png",
                        "crafting_table_front.png",
                        "crafting_table_side.png",
                        "cobblestone.png",
                    ]
                    .into_iter()
                    .map(str::to_owned),
                ),
        )
        .chain(native_core::wood::Wood::texture_names())
        .chain(DEMO_SOIL_TEXTURES.into_iter().map(str::to_owned))
        .chain(
            native_core::ore::Ore::ALL
                .into_iter()
                .map(|ore| ore.texture_name()),
        );
    for name in names {
        let path = format!("{TEXTURE_ROOT}{name}");
        let asset = sources
            .read_asset_with_metadata(&path)?
            .with_context(|| format!("missing texture {path}"))?;
        let mut reader =
            image::ImageReader::new(std::io::Cursor::new(asset.bytes)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_image_width = Some(1024);
        limits.max_image_height = Some(1024);
        limits.max_alloc = Some(32 * 1024 * 1024);
        reader.limits(limits);
        let image = reader
            .decode()
            .with_context(|| format!("invalid PNG texture {path}"))?
            .to_rgba8();
        let image = sprites::first_frame(&image, asset.metadata.as_deref())
            .with_context(|| format!("invalid texture animation {path}"))?;
        if image.width() == 0 || image.width() != image.height() {
            bail!("demo texture {path} must have a nonempty square frame");
        }
        tiles.push(image);
    }
    let tile_size = tiles.iter().map(RgbaImage::width).max().unwrap_or(16);
    let mut tiles = tiles.into_iter().map(|image| {
        if image.width() == tile_size {
            image
        } else {
            image::imageops::resize(
                &image,
                tile_size,
                tile_size,
                image::imageops::FilterType::Nearest,
            )
        }
    });
    let mut top = tiles.next().context("missing grass top texture")?;
    let mut side = tiles.next().context("missing grass side texture")?;
    let mut side_overlay = tiles.next().context("missing grass overlay texture")?;
    let dirt = tiles.next().context("missing dirt texture")?;
    let stone = tiles.next().context("missing stone texture")?;
    let stone_bricks = tiles.next().context("missing stone bricks texture")?;
    tint_grass(&mut top);
    tint_grass(&mut side_overlay);
    image::imageops::overlay(&mut side, &side_overlay, 0, 0);
    let mut atlas_tiles = vec![top, side, dirt, stone, stone_bricks];
    atlas_tiles.extend(tiles);
    make_atlas(atlas_tiles)
}

fn tint_grass(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        for (channel, tint) in pixel.0[..3].iter_mut().zip([145u16, 189, 90]) {
            *channel = ((u16::from(*channel) * tint) / 255) as u8;
        }
    }
}

fn make_atlas(tiles: Vec<RgbaImage>) -> Result<TextureAtlas> {
    if tiles.len() != DEMO_ATLAS_TILES as usize {
        bail!("incorrect number of demo atlas tiles");
    }
    let (tile_width, tile_height) = tiles[0].dimensions();
    if tile_width == 0
        || tile_height == 0
        || tiles
            .iter()
            .any(|tile| tile.dimensions() != (tile_width, tile_height))
    {
        bail!("all atlas tiles must have the same nonzero dimensions");
    }
    let width = tile_width
        .checked_mul(DEMO_ATLAS_COLUMNS)
        .context("texture atlas is too wide")?;
    let height = tile_height
        .checked_mul(DEMO_ATLAS_ROWS)
        .context("texture atlas is too tall")?;
    let mut atlas = RgbaImage::new(width, height);
    for (index, tile) in tiles.iter().enumerate() {
        let index = index as u32;
        atlas.copy_from(
            tile,
            (index % DEMO_ATLAS_COLUMNS) * tile_width,
            (index / DEMO_ATLAS_COLUMNS) * tile_height,
        )?;
    }
    Ok(TextureAtlas {
        width,
        height,
        rgba: atlas.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packs::{ResourceProvider, ResourceStack};
    use image::Rgba;
    use std::io::Write;

    #[test]
    fn atlas_keeps_distinct_tiles_and_grass_tint_preserves_alpha() {
        let mut grass = RgbaImage::from_pixel(2, 2, Rgba([255, 255, 255, 128]));
        tint_grass(&mut grass);
        assert_eq!(grass.get_pixel(0, 0).0, [145, 189, 90, 128]);
        let atlas = make_atlas(vec![grass; DEMO_ATLAS_TILES as usize]).unwrap();
        assert_eq!(atlas.width, 2 * DEMO_ATLAS_COLUMNS);
        assert_eq!(atlas.height, 2 * DEMO_ATLAS_ROWS);
        assert_eq!(atlas.rgba.len(), (atlas.width * atlas.height * 4) as usize);
    }

    #[test]
    fn missing_installation_reports_the_expected_version() {
        let path = Path::new("this-directory-does-not-exist");
        let error = load_atlas(path).unwrap_err().to_string();
        assert!(error.contains("26.3"));
    }

    #[test]
    fn packs_override_vanilla_and_later_packs_take_priority() {
        let temp = tempfile::tempdir().unwrap();
        let version = temp.path().join("versions/26.3");
        std::fs::create_dir_all(&version).unwrap();
        let jar_path = version.join("26.3.jar");
        let mut jar = zip::ZipWriter::new(File::create(&jar_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        jar.start_file("version.json", options).unwrap();
        jar.write_all(br#"{"id":"26.3","world_version":5023}"#)
            .unwrap();
        let png = |size, pixel| {
            let image =
                image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(size, size, Rgba(pixel)));
            let mut output = std::io::Cursor::new(Vec::new());
            image
                .write_to(&mut output, image::ImageFormat::Png)
                .unwrap();
            output.into_inner()
        };
        let names = [
            "grass_block_top.png",
            "grass_block_side.png",
            "grass_block_side_overlay.png",
            "dirt.png",
            "stone.png",
            "stone_bricks.png",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain(
            native_core::wood::Planks::ALL
                .into_iter()
                .map(|planks| format!("{}.png", planks.id().strip_prefix("minecraft:").unwrap()))
                .chain(
                    [
                        "crafting_table_top.png",
                        "crafting_table_front.png",
                        "crafting_table_side.png",
                        "cobblestone.png",
                    ]
                    .into_iter()
                    .map(str::to_owned),
                ),
        )
        .chain(native_core::wood::Wood::texture_names())
        .chain(DEMO_SOIL_TEXTURES.into_iter().map(str::to_owned))
        .chain(
            native_core::ore::Ore::ALL
                .into_iter()
                .map(|ore| ore.texture_name()),
        );
        for name in names {
            jar.start_file(format!("{TEXTURE_ROOT}{name}"), options)
                .unwrap();
            jar.write_all(&png(16, [100, 100, 100, 255])).unwrap();
        }
        jar.start_file(format!("{TEXTURE_ROOT}stone.png.mcmeta"), options)
            .unwrap();
        jar.write_all(br#"{"animation":{"frametime":4}}"#).unwrap();
        jar.finish().unwrap();
        let sha = format!("{:x}", Sha1::digest(std::fs::read(&jar_path).unwrap()));
        std::fs::write(
            version.join("26.3.json"),
            format!(r#"{{"id":"26.3","downloads":{{"client":{{"sha1":"{sha}"}}}}}}"#),
        )
        .unwrap();

        let first = temp.path().join("first");
        std::fs::create_dir_all(first.join(TEXTURE_ROOT)).unwrap();
        std::fs::write(
            first.join(TEXTURE_ROOT).join("stone.png"),
            png(16, [230, 0, 0, 255]),
        )
        .unwrap();
        let second = temp.path().join("second.zip");
        let mut zip = zip::ZipWriter::new(File::create(&second).unwrap());
        zip.start_file(format!("{TEXTURE_ROOT}stone.png"), options)
            .unwrap();
        zip.write_all(&png(32, [12, 234, 56, 255])).unwrap();
        zip.start_file(format!("{TEXTURE_ROOT}stone.png.mcmeta"), options)
            .unwrap();
        zip.write_all(br#"{"animation":{"frametime":3}}"#).unwrap();
        zip.start_file(format!("{TEXTURE_ROOT}stone_bricks.png"), options)
            .unwrap();
        zip.write_all(&png(32, [4, 5, 6, 255])).unwrap();
        zip.start_file(format!("{TEXTURE_ROOT}oak_planks.png"), options)
            .unwrap();
        let mut animated = RgbaImage::from_pixel(32, 64, Rgba([210, 20, 20, 255]));
        for y in 32..64 {
            for x in 0..32 {
                animated.put_pixel(x, y, Rgba([80, 160, 30, 255]));
            }
        }
        let mut animated_png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(animated)
            .write_to(&mut animated_png, image::ImageFormat::Png)
            .unwrap();
        zip.write_all(&animated_png.into_inner()).unwrap();
        zip.start_file(format!("{TEXTURE_ROOT}oak_planks.png.mcmeta"), options)
            .unwrap();
        zip.write_all(br#"{"animation":{"frames":[1,0]}}"#).unwrap();
        zip.start_file(format!("{TEXTURE_ROOT}crafting_table_top.png"), options)
            .unwrap();
        zip.write_all(&png(32, [20, 40, 210, 255])).unwrap();
        zip.finish().unwrap();
        let third = temp.path().join("metadata-only");
        std::fs::create_dir_all(third.join(TEXTURE_ROOT)).unwrap();
        std::fs::write(
            third.join(TEXTURE_ROOT).join("stone.png.mcmeta"),
            br#"{"animation":{"frametime":7}}"#,
        )
        .unwrap();
        let mut sources = ResourceStack::open(temp.path()).unwrap();
        let path = format!("{TEXTURE_ROOT}stone.png");
        assert!(sources.read_pack_asset(&path).unwrap().is_none());
        assert_eq!(
            sources
                .read_asset_with_metadata(&path)
                .unwrap()
                .unwrap()
                .metadata
                .as_deref(),
            Some(br#"{"animation":{"frametime":4}}"#.as_slice())
        );
        sources.push_pack(&first).unwrap();
        assert!(
            sources
                .read_asset_with_metadata(&path)
                .unwrap()
                .unwrap()
                .metadata
                .is_none()
        );
        sources.push_pack(&second).unwrap();
        let pack_image =
            image::load_from_memory(&sources.read_pack_asset(&path).unwrap().unwrap()).unwrap();
        assert_eq!(pack_image.width(), 32);
        assert_eq!(
            sources
                .read_asset_with_metadata(&path)
                .unwrap()
                .unwrap()
                .metadata
                .as_deref(),
            Some(br#"{"animation":{"frametime":3}}"#.as_slice())
        );
        sources.push_pack(&third).unwrap();
        assert_eq!(
            sources
                .read_asset_with_metadata(&path)
                .unwrap()
                .unwrap()
                .metadata
                .as_deref(),
            Some(br#"{"animation":{"frametime":7}}"#.as_slice())
        );
        let atlas = load_atlas_with_packs(temp.path(), &[first, second]).unwrap();
        assert_eq!(
            (atlas.width, atlas.height),
            (32 * DEMO_ATLAS_COLUMNS, 32 * DEMO_ATLAS_ROWS)
        );
        assert_eq!(&atlas.rgba[(96 * 4)..(97 * 4)], &[12, 234, 56, 255]);
        assert_eq!(&atlas.rgba[(128 * 4)..(129 * 4)], &[4, 5, 6, 255]);
        assert_eq!(&atlas.rgba[(160 * 4)..(161 * 4)], &[80, 160, 30, 255]);
        let table_top = (((DEMO_CRAFTING_TABLE_TOP / DEMO_ATLAS_COLUMNS * 32) * atlas.width
            + DEMO_CRAFTING_TABLE_TOP % DEMO_ATLAS_COLUMNS * 32)
            * 4) as usize;
        assert_eq!(&atlas.rgba[table_top..table_top + 4], &[20, 40, 210, 255]);
    }
}
