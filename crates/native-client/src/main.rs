#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accessibility_ui;
mod action_bar;
mod audio;
mod blur_ui;
mod breaking_render;
mod chat_hud;
mod chest_model;
mod chunk_stream;
mod death_ui;
mod equipment_visual;
mod experience_hud;
mod first_person;
mod fluid_mesh;
mod focus_pause;
mod fov_ui;
mod frame_pacer;
mod game_ui;
mod generated_item;
mod ground_items;
mod held_items;
mod item_gui;
mod item_render;
mod menu;
mod menu_assets;
mod menu_draw;
mod menu_gpu_worker;
mod menu_renderer;
mod menu_settings;
mod menu_software_worker;
mod mesh;
mod mesh_worker;
mod mouse_look;
mod mouse_scroll;
mod mouse_ui;
mod orb_render;
mod player_inventory;
mod player_preview;
mod player_skin;
mod preview;
mod preview_app;
mod preview_lighting;
mod recipe_book_ui;
mod renderer;
mod screen_blur;
mod sky_background;
mod sound_ui;
mod survival_hud;
mod text_component;
mod toggle_key;
mod video_ui;
mod visual_look;
#[cfg(all(test, target_os = "windows"))]
mod window_test;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use glam::{DVec3, IVec3, Vec3};
use native_persistence::nbt::{NamedTag, Tag};
use native_persistence::world::{ChunkPos, DimensionId, WorldReader};
use native_resources::transforms::{BlockTransformers, ToolKind};
use native_resources::{
    TextureAtlas,
    packs::{ResourceProvider, ResourceStack},
    recipes::CraftingRecipes,
};
#[cfg(debug_assertions)]
use native_resources::{
    default_minecraft_dir, geometry, load_atlas_with_packs, models, sprites::SpriteAtlas,
};
use native_session::{
    GameSession, LocalSession, PlayerCommand, PlayerState,
    inventory::{ClickButton, InventoryPointerEvent},
    loaded::{BuildBounds, LoadedSession},
};
#[cfg(debug_assertions)]
use std::ffi::OsStr;
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    platform::run_on_demand::EventLoopExtRunOnDemand,
    window::{CursorGrabMode, Window, WindowId},
};

use crate::renderer::Renderer;
use crate::{
    game_ui::{GameUi, PauseAction, hotbar_key, inventory_swap_key},
    menu_assets::MenuAssets,
    recipe_book_ui::BookAction,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("Minecraft Native: {error:#}");
        #[cfg(not(debug_assertions))]
        rfd::MessageDialog::new()
            .set_title("Minecraft")
            .set_description(format!("{error:#}"))
            .show();
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    #[cfg(debug_assertions)]
    if std::env::args_os().nth(1).is_some() {
        return run_cli();
    }
    menu::run()
}

#[cfg(debug_assertions)]
fn run_cli() -> Result<()> {
    let mut minecraft_dir: Option<PathBuf> = None;
    let mut check_install = false;
    let mut smoke_test = false;
    let mut inspect_world: Option<PathBuf> = None;
    let mut view_world: Option<PathBuf> = None;
    let mut play_world: Option<PathBuf> = None;
    let mut inspect_chunk: Option<(i32, i32)> = None;
    let mut inspect_dimension: Option<String> = None;
    let mut inspect_radius: Option<u32> = None;
    let mut inspect_model: Option<String> = None;
    let mut inspect_blockstate: Option<String> = None;
    let mut block_report: Option<PathBuf> = None;
    let mut properties = BTreeMap::new();
    let mut resource_packs = Vec::new();
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == OsStr::new("--minecraft-dir") {
            minecraft_dir = Some(
                args.next()
                    .context("--minecraft-dir requires a path")?
                    .into(),
            );
        } else if arg == OsStr::new("--check-install") {
            check_install = true;
        } else if arg == OsStr::new("--smoke-test") {
            smoke_test = true;
        } else if arg == OsStr::new("--resource-pack") {
            resource_packs
                .push(PathBuf::from(args.next().context(
                    "--resource-pack requires a directory or ZIP path",
                )?));
        } else if arg == OsStr::new("--block-report") {
            block_report = Some(
                args.next()
                    .context("--block-report requires a directory")?
                    .into(),
            );
        } else if arg == OsStr::new("--inspect-model") {
            let id = args
                .next()
                .context("--inspect-model requires a model identifier")?;
            inspect_model = Some(
                id.into_string()
                    .map_err(|_| anyhow::anyhow!("model identifier must be UTF-8"))?,
            );
        } else if arg == OsStr::new("--inspect-blockstate") {
            inspect_blockstate = Some(
                args.next()
                    .context("--inspect-blockstate requires a block identifier")?
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("block identifier must be UTF-8"))?,
            );
        } else if arg == OsStr::new("--property") {
            let property = args
                .next()
                .context("--property requires NAME=VALUE")?
                .into_string()
                .map_err(|_| anyhow::anyhow!("property must be UTF-8"))?;
            let (name, value) = property
                .split_once('=')
                .context("--property requires NAME=VALUE")?;
            if name.is_empty() || value.is_empty() || value.contains('=') {
                bail!("--property requires a nonempty NAME and VALUE");
            }
            if properties
                .insert(name.to_owned(), value.to_owned())
                .is_some()
            {
                bail!("duplicate property {name}");
            }
        } else if arg == OsStr::new("--inspect-world") {
            inspect_world = Some(
                args.next()
                    .context("--inspect-world requires a world directory")?
                    .into(),
            );
        } else if arg == OsStr::new("--view-world") {
            view_world = Some(
                args.next()
                    .context("--view-world requires a world directory")?
                    .into(),
            );
        } else if arg == OsStr::new("--play-world") {
            play_world = Some(
                args.next()
                    .context("--play-world requires a world directory")?
                    .into(),
            );
        } else if arg == OsStr::new("--chunk") {
            let x = args
                .next()
                .context("--chunk requires X and Z coordinates")?;
            let z = args
                .next()
                .context("--chunk requires X and Z coordinates")?;
            inspect_chunk = Some((
                x.to_str()
                    .context("chunk X must be an integer")?
                    .parse()
                    .context("chunk X must be a 32-bit integer")?,
                z.to_str()
                    .context("chunk Z must be an integer")?
                    .parse()
                    .context("chunk Z must be a 32-bit integer")?,
            ));
        } else if arg == OsStr::new("--dimension") {
            let id = args
                .next()
                .context("--dimension requires a namespaced identifier")?;
            inspect_dimension = Some(
                id.into_string()
                    .map_err(|_| anyhow::anyhow!("dimension identifier must be UTF-8"))?,
            );
        } else if arg == OsStr::new("--radius") {
            let radius = args
                .next()
                .context("--radius requires a nonnegative chunk radius")?;
            inspect_radius = Some(
                radius
                    .to_str()
                    .context("radius must be an integer")?
                    .parse()
                    .context("radius must be a nonnegative integer")?,
            );
        } else if arg == OsStr::new("--help") || arg == OsStr::new("-h") {
            println!(
                "Minecraft Native demo\n\nUsage: native-client [--minecraft-dir PATH] [--resource-pack PATH] [--check-install] [--smoke-test]\n       native-client [--minecraft-dir PATH] [--resource-pack PATH] --inspect-model ID\n       native-client [--minecraft-dir PATH] [--resource-pack PATH] --inspect-blockstate ID [--property NAME=VALUE]...\n       native-client --inspect-world PATH [--dimension ID] [--chunk X Z [--radius N]]\n       native-client [--minecraft-dir PATH] [--resource-pack PATH] (--view-world | --play-world) PATH [--block-report DIR] [--dimension ID] [--chunk X Z] [--radius N] [--smoke-test]\n\nWorld play is experimental and never saves. Demo: WASD, mouse, Space, Ctrl or double-tap W to sprint, Shift to sneak, left/right click, 1-9; Escape releases mouse."
            );
            return Ok(());
        } else {
            bail!("unknown argument: {} (use --help)", arg.to_string_lossy());
        }
    }
    if view_world.is_some() && play_world.is_some() {
        bail!("--view-world and --play-world cannot be combined");
    }
    let play = play_world.is_some();
    if let Some(world_dir) = play_world.or(view_world) {
        if inspect_world.is_some()
            || check_install
            || inspect_model.is_some()
            || inspect_blockstate.is_some()
            || !properties.is_empty()
        {
            bail!(
                "world viewing or play cannot be combined with inspection or installation checks"
            );
        }
        let minecraft_dir = match minecraft_dir {
            Some(path) => path,
            None => default_minecraft_dir()?,
        };
        let mut event_loop = EventLoop::new().context("cannot create the Windows event loop")?;
        return view_existing_world(
            &mut event_loop,
            &world_dir,
            &minecraft_dir,
            &resource_packs,
            ViewOptions {
                dimension: inspect_dimension.as_deref(),
                chunk: inspect_chunk,
                radius: inspect_radius.unwrap_or(1),
                smoke_test,
                block_report: block_report.as_deref(),
                play,
                video: menu_settings::VideoSettings::default(),
            },
            None,
        )
        .map(|_| ());
    }
    if block_report.is_some() {
        bail!("--block-report requires --view-world or --play-world");
    }
    if let Some(world_dir) = inspect_world {
        if check_install
            || smoke_test
            || minecraft_dir.is_some()
            || inspect_model.is_some()
            || inspect_blockstate.is_some()
            || !properties.is_empty()
            || !resource_packs.is_empty()
        {
            bail!("--inspect-world cannot be combined with graphics or installation options");
        }
        if inspect_radius.is_some() && inspect_chunk.is_none() {
            bail!("--radius requires --chunk");
        }
        let dimension = DimensionId::parse(
            inspect_dimension
                .as_deref()
                .unwrap_or("minecraft:overworld"),
        )?;
        return inspect(
            &world_dir,
            dimension,
            inspect_chunk,
            inspect_radius.unwrap_or(0),
        );
    }
    if inspect_chunk.is_some() || inspect_dimension.is_some() || inspect_radius.is_some() {
        bail!(
            "--chunk, --dimension and --radius require --inspect-world, --view-world or --play-world"
        );
    }
    if !properties.is_empty() && inspect_blockstate.is_none() {
        bail!("--property requires --inspect-blockstate");
    }
    let minecraft_dir = match minecraft_dir {
        Some(path) => path,
        None => default_minecraft_dir()?,
    };
    if let Some(id) = inspect_model {
        if check_install || smoke_test || inspect_blockstate.is_some() {
            bail!("--inspect-model cannot be combined with graphics or installation checks");
        }
        let mut sources = ResourceStack::open(&minecraft_dir)?;
        for path in &resource_packs {
            sources.push_pack(path)?;
        }
        let model = models::resolve_model(&mut sources, &id)?;
        let elements = model
            .fields
            .get("elements")
            .and_then(|value| value.as_array())
            .map_or(0, Vec::len);
        let textures = model
            .fields
            .get("textures")
            .and_then(|value| value.as_object())
            .map_or(0, |value| value.len());
        println!(
            "Model {id}: {elements} elements, {textures} texture references after inheritance"
        );
        return Ok(());
    }
    if let Some(id) = inspect_blockstate {
        if check_install || smoke_test {
            bail!("--inspect-blockstate cannot be combined with graphics or installation checks");
        }
        let mut sources = ResourceStack::open(&minecraft_dir)?;
        for path in &resource_packs {
            sources.push_pack(path)?;
        }
        let groups = geometry::bake_blockstate(&mut sources, &id, &properties)?.groups;
        let variants: usize = groups.iter().map(Vec::len).sum();
        let quads: usize = groups
            .iter()
            .flatten()
            .map(|model| model.model.quads.len())
            .sum();
        let textures: Vec<_> = groups
            .iter()
            .flatten()
            .flat_map(|model| &model.model.quads)
            .map(|quad| quad.texture.sprite.clone())
            .collect();
        if textures.is_empty() {
            println!(
                "Blockstate {id}: {} model groups, {variants} variants, {quads} quads, no textures",
                groups.len()
            );
        } else {
            let atlas = SpriteAtlas::build(&mut sources, textures, 4096)?;
            println!(
                "Blockstate {id}: {} model groups, {variants} variants, {quads} quads; atlas {}x{}",
                groups.len(),
                atlas.texture.width,
                atlas.texture.height
            );
        }
        return Ok(());
    }
    let atlas = load_atlas_with_packs(&minecraft_dir, &resource_packs)?;
    if check_install {
        println!(
            "Minecraft 26.3 verified: {} (atlas {}x{})",
            minecraft_dir.display(),
            atlas.width,
            atlas.height
        );
        return Ok(());
    }
    let mut event_loop = EventLoop::new().context("cannot create the Windows event loop")?;
    let mut resources = ResourceStack::open(&minecraft_dir)?;
    let tools = load_tool_transformers(&mut resources)?;
    let loot = load_block_loot(&mut resources, &tools)?;
    let enchantments = native_resources::enchantments::OxygenEnchantments::load(&mut resources)?;
    run_demo(
        &mut event_loop,
        atlas,
        smoke_test,
        None,
        SessionContent {
            tools: tools.map(Some),
            loot: Some(Arc::new(loot)),
            enchantments: Some(Arc::new(enchantments)),
            ..Default::default()
        },
        menu_settings::VideoSettings::default(),
        None,
    )
    .map(|_| ())
}

#[derive(Default)]
struct SessionContent {
    recipes: Option<Arc<CraftingRecipes>>,
    loot: Option<Arc<native_resources::loot::BlockLootTables>>,
    enchantments: Option<Arc<native_resources::enchantments::OxygenEnchantments>>,
    tools: [Option<Arc<BlockTransformers>>; 3],
}

fn load_tool_transformers(resources: &mut ResourceStack) -> Result<[Arc<BlockTransformers>; 3]> {
    Ok([
        Arc::new(BlockTransformers::load_for(resources, ToolKind::Axe)?),
        Arc::new(BlockTransformers::load_for(resources, ToolKind::Shovel)?),
        Arc::new(BlockTransformers::load_for(resources, ToolKind::Hoe)?),
    ])
}

fn load_block_loot(
    resources: &mut ResourceStack,
    tools: &[Arc<BlockTransformers>; 3],
) -> Result<native_resources::loot::BlockLootTables> {
    let mut loot = native_resources::loot::BlockLootTables::load(resources)?;
    loot.load_interactions(
        resources,
        tools.iter().flat_map(|tool| tool.referenced_loot()),
    )?;
    Ok(loot)
}

#[derive(Clone, Copy, Debug)]
enum GameExit {
    ReturnToTitle(menu_settings::VideoSettings),
    Quit,
}

fn run_demo(
    event_loop: &mut EventLoop<()>,
    atlas: TextureAtlas,
    smoke_test: bool,
    assets: Option<Arc<MenuAssets>>,
    content: SessionContent,
    video: menu_settings::VideoSettings,
    audio: Option<audio::UiAudio>,
) -> Result<GameExit> {
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(atlas, smoke_test, assets, content, video);
    app.audio = audio.clone();
    if let Some(ui) = &mut app.ui {
        ui.set_audio(audio);
    }
    event_loop
        .run_app_on_demand(&mut app)
        .context("window event loop failed")?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(if app.quit_application {
        GameExit::Quit
    } else {
        GameExit::ReturnToTitle(app.video)
    })
}

struct SavedCamera {
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
    dimension: Option<String>,
}

fn saved_camera(player: &NamedTag) -> Result<Option<SavedCamera>> {
    let Some(position) = native_session::player_data::read_saved_position(player)? else {
        return Ok(None);
    };
    let position = position.to_array();
    let (yaw, pitch) = match player.value.get("Rotation") {
        Some(Tag::List { values, .. }) => match values.as_slice() {
            [Tag::Float(yaw), Tag::Float(pitch)] if yaw.is_finite() && pitch.is_finite() => {
                (*yaw, *pitch)
            }
            _ => bail!("player Rotation must contain two finite floats"),
        },
        None => (0.0, 0.0),
        _ => bail!("player Rotation must be a list"),
    };
    let dimension = match player.value.get("Dimension") {
        Some(Tag::String(value)) => {
            Some(String::from_utf16(&value.0).context("player dimension contains invalid UTF-16")?)
        }
        None => None,
        _ => bail!("player Dimension must be a string"),
    };
    Ok(Some(SavedCamera {
        position,
        yaw,
        pitch,
        dimension,
    }))
}

struct ViewOptions<'a> {
    dimension: Option<&'a str>,
    chunk: Option<(i32, i32)>,
    radius: u32,
    smoke_test: bool,
    block_report: Option<&'a std::path::Path>,
    play: bool,
    video: menu_settings::VideoSettings,
}

struct PreparedPlayWorld {
    scene: preview::PreviewScene,
    session: LoadedSession,
    resources: ResourceStack,
    streamer: chunk_stream::ChunkStreamer,
    smoke_test: bool,
    assets: Option<Arc<MenuAssets>>,
    player_skin: Option<Arc<player_skin::PlayerSkin>>,
    trims: Arc<equipment_visual::TrimRegistry>,
    sky_timeline: Option<preview_lighting::SkyTimeline>,
    sky_background: Option<sky_background::SkyBackground>,
    video: menu_settings::VideoSettings,
}

enum PreparedWorld {
    Play(Box<PreparedPlayWorld>),
    #[cfg(debug_assertions)]
    Preview {
        scene: preview::PreviewScene,
        camera: preview_app::Camera,
        smoke_test: bool,
        video: menu_settings::VideoSettings,
    },
}

#[cfg(debug_assertions)]
fn view_existing_world(
    event_loop: &mut EventLoop<()>,
    world_dir: &std::path::Path,
    minecraft_dir: &std::path::Path,
    packs: &[PathBuf],
    options: ViewOptions<'_>,
    assets: Option<Arc<MenuAssets>>,
) -> Result<GameExit> {
    let prepared = prepare_existing_world(world_dir, minecraft_dir, packs, options, assets)?;
    run_prepared_world(event_loop, prepared, None)
}

fn run_prepared_world(
    event_loop: &mut EventLoop<()>,
    prepared: PreparedWorld,
    audio: Option<audio::UiAudio>,
) -> Result<GameExit> {
    match prepared {
        PreparedWorld::Play(world) => {
            let PreparedPlayWorld {
                scene,
                session,
                resources,
                streamer,
                smoke_test,
                assets,
                player_skin,
                trims,
                sky_timeline,
                sky_background,
                video,
            } = *world;
            preview_app::run_play(
                event_loop,
                scene,
                session,
                resources,
                streamer,
                smoke_test,
                assets,
                player_skin,
                trims,
                sky_timeline,
                sky_background,
                video,
                audio,
            )
        }
        #[cfg(debug_assertions)]
        PreparedWorld::Preview {
            scene,
            camera,
            smoke_test,
            video,
        } => preview_app::run(event_loop, scene, camera, smoke_test, video),
    }
}

/// All disk reads and CPU scene preparation are independent of the window
/// event loop; GUI launches run this on a background thread.
fn prepare_existing_world(
    world_dir: &std::path::Path,
    minecraft_dir: &std::path::Path,
    packs: &[PathBuf],
    options: ViewOptions<'_>,
    assets: Option<Arc<MenuAssets>>,
) -> Result<PreparedWorld> {
    let ViewOptions {
        dimension,
        chunk,
        radius,
        smoke_test,
        block_report,
        play,
        video,
    } = options;
    let mut reader = WorldReader::open_for_session(
        world_dir,
        DimensionId::parse(dimension.unwrap_or("minecraft:overworld"))?,
    )?;
    let player_data = match reader.read_player() {
        Ok(player) => player,
        Err(error) if chunk.is_some() => {
            eprintln!("Saved player data could not be read; using the selected chunk: {error:#}");
            None
        }
        Err(error) => return Err(error),
    };
    let player = player_data
        .as_ref()
        .map(saved_camera)
        .transpose()?
        .flatten();
    if dimension.is_none()
        && let Some(saved) = &player
        && let Some(id) = &saved.dimension
    {
        reader.set_dimension(DimensionId::parse(id)?);
    }
    if chunk.is_none()
        && let Some(saved) = &player
        && let Some(saved_dimension) = &saved.dimension
        && DimensionId::parse(saved_dimension)? != *reader.dimension()
    {
        #[cfg(debug_assertions)]
        bail!("saved player is in {saved_dimension}; pass --chunk X Z to view another dimension");
        #[cfg(not(debug_assertions))]
        bail!("saved player is in another dimension: {saved_dimension}");
    }
    let center = match (chunk, &player) {
        (Some((x, z)), _) => ChunkPos { x, z },
        (None, Some(player)) => ChunkPos {
            x: (player.position[0].floor() as i32).div_euclid(16),
            z: (player.position[2].floor() as i32).div_euclid(16),
        },
        (None, None) => {
            #[cfg(debug_assertions)]
            bail!("world has no saved player position; pass --chunk X Z to select an area");
            #[cfg(not(debug_assertions))]
            bail!("world has no saved player position and cannot yet be opened");
        }
    };
    // Load only the immediate playable area before opening the window; the
    // streamer fills the configured render distance in the background.
    let area = reader.read_area(center, if play { radius.min(1) } else { radius })?;
    if area.chunks.is_empty() {
        bail!("no saved chunks found in the selected area");
    }
    let defaults = if let Some(directory) = block_report {
        let defaults = native_resources::block_registry::load(minecraft_dir, directory)
            .context("cannot load block registry report")?;
        println!("Loaded default states for {} blocks", defaults.len());
        defaults
    } else {
        BTreeMap::new()
    };
    let mut world = area.materialize_with_defaults(reader.dimension(), &defaults)?;
    let origin = IVec3::new(
        center
            .x
            .checked_mul(16)
            .context("chunk X outside supported range")?,
        0,
        center
            .z
            .checked_mul(16)
            .context("chunk Z outside supported range")?,
    );
    let camera = if chunk.is_none() {
        let player = player
            .as_ref()
            .context("world has no saved player position")?;
        preview_app::Camera {
            eye: Vec3::new(
                (player.position[0] - f64::from(origin.x)) as f32,
                (player.position[1] + 1.62) as f32,
                (player.position[2] - f64::from(origin.z)) as f32,
            ),
            yaw: (player.yaw + 180.0).to_radians(),
            pitch: (-player.pitch)
                .to_radians()
                .clamp(-native_session::MAX_PITCH, native_session::MAX_PITCH),
        }
    } else {
        let mut surface = None;
        if let Some(chunk) = world.chunks.get(&native_world::loaded::ChunkPos {
            x: center.x,
            z: center.z,
        }) {
            for (&y, section) in chunk.sections.iter().rev() {
                if let Some(blocks) = &section.blocks {
                    for local_y in (0..16).rev() {
                        let id = blocks[8 + 8 * 16 + local_y * 256];
                        if world.state(id).is_some_and(|state| {
                            !matches!(
                                state.name.as_str(),
                                "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                            )
                        }) {
                            surface = Some(i32::from(y) * 16 + local_y as i32);
                            break;
                        }
                    }
                }
                if surface.is_some() {
                    break;
                }
            }
        }
        preview_app::Camera {
            eye: Vec3::new(8.5, surface.unwrap_or(72) as f32 + 8.0, 15.5),
            yaw: 0.0,
            pitch: -0.65,
        }
    };
    let mut resources = ResourceStack::open(minecraft_dir)?;
    for pack in packs {
        resources.push_pack(pack)?;
    }
    let saved_packs = reader.data_pack_settings()?;
    let pack_warnings = resources.configure_world_data(
        world_dir,
        &native_resources::datapacks::DataPackSettings {
            enabled: saved_packs.enabled,
            disabled: saved_packs.disabled,
            enabled_features: saved_packs.enabled_features,
        },
    )?;
    let saved_dimensions = reader
        .saved_dimension_types()
        .context("cannot read saved dimension mappings")?;
    world.dimension_type = preview_lighting::resolve_dimension_type(
        &mut resources,
        reader.dimension(),
        saved_dimensions.as_ref(),
    )
    .context("cannot resolve the world's dimension type")?;
    if play {
        let profile = match &world.dimension_type {
            Some(id) => preview_lighting::PreviewLighting::load_type(&mut resources, id),
            None => preview_lighting::PreviewLighting::load(&mut resources, &world.dimension),
        }
        .context("cannot determine dimension bounds for experimental play")?;
        let (min_y, max_y) = profile
            .build_bounds
            .context("dimension type is missing min_y or height")?;
        let initial_player = PlayerState {
            position: camera.eye - Vec3::Y * native_session::EYE_HEIGHT,
            yaw: camera.yaw,
            pitch: camera.pitch,
            vertical_velocity: 0.0,
            on_ground: false,
            pose: native_session::PlayerPose::Standing,
        };
        let rules = reader
            .fluid_game_rules()
            .context("cannot load world game rules")?;
        let difficulty = reader
            .difficulty_settings()
            .context("cannot load world difficulty")?;
        let mut heightmap_tags = match resources.vanilla_block_ids().and_then(|registry| {
            native_resources::heightmap_tags::HeightmapTags::load(&mut resources, &registry)
        }) {
            Ok(tags) => Some(Arc::new(tags)),
            Err(error) => {
                eprintln!("Spawn heightmaps are unavailable: {error:#}");
                None
            }
        };
        let fast_lava = world.dimension_type.as_deref().and_then(|dimension_type| {
            match native_resources::fluid_environment::load_fast_lava_environment(
                &mut resources,
                dimension_type,
            ) {
                Ok(value) => Some(value),
                Err(error) => {
                    eprintln!(
                        "Fast lava environment is unavailable; using dimension fallback: {error:#}"
                    );
                    None
                }
            }
        });
        let sky_timeline = world.dimension_type.as_deref().and_then(|dimension_type| {
            match preview_lighting::SkyTimeline::load(&mut resources, dimension_type) {
                Ok(timeline) if timeline.is_animated() => Some(timeline),
                Ok(_) => None,
                Err(error) => {
                    eprintln!("Sky light timeline unavailable: {error:#}");
                    None
                }
            }
        });
        let sky_background = world.dimension_type.as_deref().and_then(|dimension_type| {
            match sky_background::SkyBackground::load(&mut resources, dimension_type, min_y, max_y)
            {
                Ok(background) => background,
                Err(error) => {
                    eprintln!("Sky background is unavailable: {error:#}");
                    None
                }
            }
        });
        let mut session =
            LoadedSession::new(world, origin, BuildBounds { min_y, max_y }, initial_player)
                .map_err(anyhow::Error::msg)?;
        session
            .set_has_skylight(profile.has_skylight)
            .map_err(anyhow::Error::msg)?;
        let clocks = reader.world_clocks_document().and_then(|source| {
            let mut clocks = source
                .as_ref()
                .map(native_persistence::world::parse_world_clocks)
                .transpose()?
                .unwrap_or_default();
            for id in resources.list_data("world_clock")? {
                let id = native_persistence::world::DimensionId::parse(&format!(
                    "{}:{}",
                    id.namespace, id.path
                ))?;
                clocks.entry(id).or_default();
            }
            Ok((clocks, source))
        });
        match clocks {
            Ok((clocks, Some(source))) => {
                if let Err(error) =
                    session.restore_saved_world_clocks(&source, clocks, rules.advance_time)
                {
                    eprintln!("World clocks are unavailable: {error:#}");
                }
            }
            Ok((clocks, None)) => session.set_world_clocks(clocks, rules.advance_time),
            Err(error) => eprintln!("World clocks are unavailable: {error:#}"),
        }
        if let Some(environment) = fast_lava
            && let Err(error) = session.set_fast_lava_environment(Arc::new(environment))
        {
            eprintln!("Fast lava environment is unavailable; using dimension fallback: {error}");
        }
        session.set_player_uuid(reader.singleplayer_uuid_words()?);
        let active_enchantments =
            native_resources::enchantments::OxygenEnchantments::load(&mut resources).map(Arc::new);
        if let Ok(damage_types) = &active_enchantments {
            session.set_orb_damage_tags(Arc::clone(damage_types));
        }
        match resources.vanilla_item_registry() {
            Ok(items) => {
                let items = Arc::new(items);
                session.set_item_registry(Arc::clone(&items));
                match &active_enchantments {
                    Ok(damage_types) => {
                        session.set_item_damage_rules(items, Arc::clone(damage_types))
                    }
                    Err(error) => eprintln!("Item damage rules are unavailable: {error:#}"),
                }
            }
            Err(error) => eprintln!("Item registry is unavailable: {error:#}"),
        }
        match resources.vanilla_entity_collision_registry() {
            Ok(registry) => session.set_entity_collision_registry(Arc::new(registry))?,
            Err(error) => eprintln!("Spawn entity collision registry is unavailable: {error:#}"),
        }
        if let Some(tags) = &heightmap_tags
            && let Err(error) = session.set_spawn_heightmap_tags(Arc::clone(tags))
        {
            eprintln!("Spawn heightmaps are unavailable: {error:#}");
            heightmap_tags = None;
        }
        for (pos, saved) in &area.chunks {
            let pos = native_world::loaded::ChunkPos { x: pos.x, z: pos.z };
            let entities = chunk_stream::read_entity_data(&mut reader, pos);
            session.set_initial_entity_chunk(pos, entities)?;
            let poi = chunk_stream::read_poi_data(&mut reader, pos);
            session.set_initial_poi_chunk(pos, poi)?;
            if heightmap_tags.is_some()
                && let Err(error) = session.prepare_initial_spawn_heightmaps(saved)
            {
                eprintln!(
                    "Could not prepare spawn heightmaps for chunk {}, {}: {error:#}",
                    pos.x, pos.z
                );
            }
        }
        if chunk.is_none() {
            let saved = player_data
                .as_ref()
                .context("world has no saved player data")?;
            let saved_camera = player
                .as_ref()
                .context("world has no saved player position")?;
            let position = DVec3::new(
                saved_camera.position[0] - f64::from(origin.x),
                saved_camera.position[1],
                saved_camera.position[2] - f64::from(origin.z),
            );
            let motion = native_session::player_data::read_saved_motion(saved)
                .context("cannot read saved player motion")?;
            session
                .restore_saved_motion(position, motion)
                .map_err(anyhow::Error::msg)?;
        }
        let default_mode = native_session::GameMode::from_legacy_id(reader.default_game_type_id()?);
        let mode = player_data
            .as_ref()
            .and_then(player_inventory::saved_game_mode)
            .unwrap_or(default_mode);
        session.set_game_mode(mode);
        if matches!(
            mode,
            native_session::GameMode::Spectator | native_session::GameMode::Creative
        ) && let Some(saved) = &player_data
        {
            let speed = native_session::player_data::read_saved_flight_speed(saved)
                .context("cannot read saved player flight speed")?;
            session
                .restore_saved_flight_speed(speed)
                .map_err(anyhow::Error::msg)?;
            session
                .restore_saved_walking_speed(native_session::player_data::read_saved_walking_speed(
                    saved,
                )?)
                .map_err(anyhow::Error::msg)?;
            session
                .restore_saved_flying(native_session::player_data::read_saved_flying(saved))
                .map_err(anyhow::Error::msg)?;
        }
        session.set_game_master_permission(reader.allow_commands()?);
        match resources.vanilla_block_ids() {
            Ok(registry) => {
                session.set_vanilla_block_ids(Arc::new(registry.clone()));
                let tags = native_resources::mining_tags::MiningTags::load(&mut resources, &registry)
                    .context("월드의 도구 채굴 태그를 읽을 수 없습니다. 활성 데이터팩과 Minecraft 26.3 설치를 확인하세요")?;
                session.set_mining_tags(Arc::new(tags));
                match native_resources::respawn_tags::load_fire_tag(&mut resources, &registry) {
                    Ok(tag) => session.set_falling_fire_tag(Arc::new(tag)),
                    Err(error) => eprintln!("Falling-block fire tag is unavailable: {error:#}"),
                }
                match native_resources::respawn_tags::load_bounce_suppression_tag(
                    &mut resources,
                    &registry,
                ) {
                    Ok(tag) => session.set_bounce_suppression_tag(Arc::new(tag)),
                    Err(error) => eprintln!("Bounce suppression tag is unavailable: {error:#}"),
                }
                match native_resources::respawn_tags::load_farmland_maintainers(
                    &mut resources,
                    &registry,
                ) {
                    Ok(tag) => session.set_farmland_maintainers(Arc::new(tag)),
                    Err(error) => eprintln!("Farmland maintenance tag is unavailable: {error:#}"),
                }
                match native_resources::respawn_tags::SnowSupportTags::load(
                    &mut resources,
                    &registry,
                ) {
                    Ok(tags) => session.set_snow_support_tags(Arc::new(tags)),
                    Err(error) => eprintln!("Snow support tags are unavailable: {error:#}"),
                }
                match native_resources::respawn_tags::load_snow_tag(&mut resources, &registry) {
                    Ok(tag) => session.set_snowy_tag(Arc::new(tag)),
                    Err(error) => eprintln!("Snowy block tag is unavailable: {error:#}"),
                }
            }
            Err(error) => {
                return Err(error.context(
                    "월드의 블록 등록 정보를 읽을 수 없습니다. Minecraft 26.3 설치를 확인하세요",
                ));
            }
        }
        session.set_survival_rules(native_session::survival::SurvivalRules {
            difficulty: difficulty.difficulty,
            natural_health_regeneration: rules.natural_health_regeneration,
            fall_damage: rules.fall_damage,
        });
        session.set_random_tick_speed(rules.random_tick_speed as u32);
        session.set_death_rules(native_session::DeathRules {
            keep_inventory: rules.keep_inventory,
            immediate_respawn: rules.immediate_respawn,
            show_death_messages: rules.show_death_messages,
        });
        if let Some(player_data) = &player_data {
            session.set_player_experience(native_session::player_data::read_saved_experience(
                player_data,
            ));
            session.set_player_score(
                player_data
                    .value
                    .get("Score")
                    .and_then(native_persistence::nbt::Tag::as_java_int)
                    .unwrap_or(0),
            );
            session.set_player_vitals(
                native_session::player_data::read_saved_vitals(player_data, difficulty.hardcore)
                    .context("cannot read saved player health and food")?,
            );
            session
                .restore_saved_player_inventory(player_data)
                .context("cannot read saved player inventory")?;
            session
                .set_saved_breathing(
                    native_session::player_data::read_saved_breathing_effects(player_data)
                        .context("cannot read saved breathing effects")?,
                    native_session::player_data::read_saved_oxygen_attribute(player_data)
                        .context("cannot read saved oxygen attribute")?,
                    active_enchantments
                        .as_ref()
                        .map(Arc::clone)
                        .map_err(|error| {
                            anyhow::anyhow!(
                                "cannot load enchantments from the active data packs: {error:#}"
                            )
                        })?,
                )
                .context("cannot evaluate oxygen equipment")?;
            let (armor, toughness) =
                native_session::player_data::read_saved_armor_attributes(player_data)
                    .context("cannot read saved armor attributes")?;
            session
                .set_saved_armor(armor, toughness)
                .context("cannot evaluate armor equipment")?;
            let (safe, multiplier) =
                native_session::player_data::read_saved_fall_attributes(player_data)
                    .context("cannot read saved fall attributes")?;
            session
                .set_saved_bounciness(
                    native_session::player_data::read_saved_bounciness_attribute(player_data)
                        .context("cannot read saved bounciness")?,
                )
                .context("cannot restore saved bounciness")?;
            session
                .set_saved_jump_strength(
                    native_session::player_data::read_saved_jump_strength_attribute(player_data)
                        .context("cannot read saved jump strength")?,
                )
                .context("cannot restore saved jump strength")?;
            session
                .restore_saved_gravity(
                    native_session::player_data::read_saved_gravity_attribute(player_data)
                        .context("cannot read saved gravity")?,
                    native_session::player_data::read_saved_no_gravity(player_data),
                )
                .context("cannot restore saved gravity")?;
            session
                .restore_saved_water_movement_efficiency(
                    native_session::player_data::read_saved_water_movement_efficiency_attribute(
                        player_data,
                    )
                    .context("cannot read saved water movement efficiency")?,
                )
                .context("cannot restore saved water movement efficiency")?;
            session
                .restore_saved_mining_attributes(
                    native_session::player_data::read_saved_mining_attributes(player_data)
                        .context("cannot read saved mining attributes")?,
                )
                .context("cannot restore saved mining attributes")?;
            let fall_effects = native_session::player_data::read_saved_fall_effects(player_data)
                .context("cannot read saved fall effects")?;
            session
                .set_saved_fall(safe, multiplier, fall_effects)
                .context("cannot configure fall damage")?;
            session
                .set_saved_movement_speed(
                    native_session::player_data::read_loaded_movement_speed_attribute(player_data)
                        .context("cannot read saved movement speed")?,
                )
                .context("cannot restore saved movement speed")?;

            let mut prepare_respawn =
                || -> Result<Option<Option<native_session::loaded::SavedRespawnSupport>>> {
                    let Some(config) = native_persistence::world::read_player_respawn(player_data)?
                    else {
                        return Ok(Some(None));
                    };
                    if config.data.dimension != *reader.dimension() {
                        return Ok(None);
                    }
                    let Some(dimension_type) = session
                        .snapshot()
                        .world
                        .loaded()
                        .and_then(|world| world.dimension_type.clone())
                    else {
                        return Ok(None);
                    };
                    let environment = Arc::new(
                        native_resources::respawn_environment::RespawnEnvironmentRegistry::load(
                            &mut resources,
                            &dimension_type,
                        )?,
                    );
                    let blocks = resources.vanilla_block_ids()?;
                    let tags = Arc::new(native_resources::respawn_tags::RespawnBlockTags::load(
                        &mut resources,
                        blocks,
                    )?);
                    Ok(Some(Some(native_session::loaded::SavedRespawnSupport {
                        config,
                        environment,
                        tags,
                        border: reader.world_border()?,
                        max_health: native_session::player_data::read_respawn_max_health(
                            player_data,
                        )?,
                    })))
                };
            match prepare_respawn() {
                Ok(Some(Some(support))) => session.set_saved_respawn_support(support)?,
                Ok(Some(None)) => session.set_saved_respawn_absent()?,
                Ok(None) => {}
                Err(error) => eprintln!("Saved respawn is unavailable: {error:#}"),
            }
            let prepare_world_respawn =
                || -> Result<Option<native_session::loaded::WorldRespawnSupport>> {
                    let spawn = reader.world_spawn()?;
                    if spawn.dimension != *reader.dimension()
                        || (default_mode != native_session::GameMode::Adventure
                            && profile.has_ceiling != Some(false))
                    {
                        return Ok(None);
                    }
                    Ok(Some(native_session::loaded::WorldRespawnSupport {
                        spawn,
                        border: reader.world_border()?,
                        max_health: native_session::player_data::read_respawn_max_health(
                            player_data,
                        )?,
                        radius: rules.respawn_radius,
                        adventure: default_mode == native_session::GameMode::Adventure,
                        ceiling_spawn_height: None,
                    }))
                };
            match prepare_world_respawn() {
                Ok(Some(support)) => session.set_world_respawn_support(support)?,
                Ok(None) => {}
                Err(error) => eprintln!("World respawn is unavailable: {error:#}"),
            }
        } else {
            session.set_player_vitals(native_session::survival::PlayerVitals {
                hardcore: difficulty.hardcore,
                ..Default::default()
            });
        }
        let recipes =
            CraftingRecipes::load(&mut resources).context("cannot load crafting recipes")?;
        for warning in recipes.warnings.iter().take(20) {
            eprintln!("recipe issue: {warning}");
        }
        session.set_crafting_recipes(Arc::new(recipes));
        let transforms = load_tool_transformers(&mut resources)
            .context("cannot load tool block transformations")?;
        let loot = load_block_loot(&mut resources, &transforms)
            .context("cannot load block loot tables")?;
        for warning in loot.warnings.iter().take(20) {
            eprintln!("loot issue: {warning}");
        }
        session.set_block_loot(Arc::new(loot));
        for (kind, transforms) in [ToolKind::Axe, ToolKind::Shovel, ToolKind::Hoe]
            .into_iter()
            .zip(transforms)
        {
            session.set_tool_transformers(kind, transforms);
        }
        session.set_block_drops(rules.block_drops);
        session.set_entity_drops(rules.entity_drops);
        session.set_source_conversion(rules.water_source_conversion, rules.lava_source_conversion);
        let player_skin = if assets.is_some() {
            let uuid = reader.singleplayer_uuid()?;
            Some(Arc::new(player_skin::PlayerSkin::load(
                &mut resources,
                uuid.as_deref(),
            )?))
        } else {
            None
        };
        if session.unsupported_states() != 0 {
            eprintln!(
                "{} block states use conservative full-block collision because their shapes are not implemented",
                session.unsupported_states()
            );
        }
        if session.unsupported_outlines() != 0 {
            eprintln!(
                "{} block states use conservative full-block targeting because their outlines are not implemented",
                session.unsupported_outlines()
            );
        }
        let snapshot = session.snapshot();
        let loaded_world = snapshot
            .world
            .loaded()
            .context("saved-world session has no loaded world")?;
        let mut scene = preview::PreviewScene::prepare(loaded_world, &mut resources, origin, 4096)?;
        scene.warnings.extend(pack_warnings);
        for warning in scene.warnings.iter().take(20) {
            eprintln!("preview issue: {warning}");
        }
        println!(
            "Experimental unsaved world play: {} loaded chunks, {} missing; changes are discarded on exit",
            area.chunks.len(),
            area.missing.len()
        );
        let streamer = chunk_stream::ChunkStreamer::start(
            reader,
            defaults,
            radius,
            loaded_world.missing_chunks.clone(),
            heightmap_tags.map(|tags| {
                (
                    native_persistence::heightmap::HeightBounds { min_y, max_y },
                    tags,
                )
            }),
        )?;
        let trims = Arc::new(
            equipment_visual::TrimRegistry::load(&mut resources)
                .context("cannot load armor trim definitions")?,
        );
        return Ok(PreparedWorld::Play(Box::new(PreparedPlayWorld {
            scene,
            session,
            resources,
            streamer,
            smoke_test,
            assets,
            player_skin,
            trims,
            sky_timeline,
            sky_background,
            video,
        })));
    }
    #[cfg(debug_assertions)]
    {
        let mut scene = preview::PreviewScene::prepare(&world, &mut resources, origin, 4096)?;
        scene.warnings.extend(pack_warnings);
        println!(
            "Read-only preview: {}:{}; {} loaded chunks, {} missing; {} opaque vertices, {} translucent quads",
            reader.dimension().namespace,
            reader.dimension().path,
            area.chunks.len(),
            area.missing.len(),
            scene.opaque_len(),
            scene.translucent_len()
        );
        if !scene.warnings.is_empty() {
            eprintln!("{} preview issues:", scene.warnings.len());
            for warning in scene.warnings.iter().take(20) {
                eprintln!("  {warning}");
            }
            if scene.warnings.len() > 20 {
                eprintln!("  ... and {} more", scene.warnings.len() - 20);
            }
        }
        Ok(PreparedWorld::Preview {
            scene,
            camera,
            smoke_test,
            video,
        })
    }
    #[cfg(not(debug_assertions))]
    bail!("Unsupported world mode")
}

#[cfg(debug_assertions)]
fn inspect(
    world_dir: &std::path::Path,
    dimension: DimensionId,
    chunk: Option<(i32, i32)>,
    radius: u32,
) -> Result<()> {
    let mut reader = WorldReader::open(world_dir, dimension)?;
    println!(
        "World data version: {} (Minecraft 26.3: 5023); dimension {}:{}",
        reader.info.data_version,
        reader.dimension().namespace,
        reader.dimension().path
    );
    if let Some((x, z)) = chunk {
        let area = reader.read_area(ChunkPos { x, z }, radius)?;
        let block_sections = area
            .chunks
            .values()
            .flat_map(|chunk| &chunk.sections)
            .filter(|section| section.blocks.is_some())
            .count();
        let palettes: usize = area
            .chunks
            .values()
            .flat_map(|chunk| &chunk.sections)
            .filter_map(|section| section.blocks.as_ref())
            .map(|blocks| blocks.states.len())
            .sum();
        let full = area.chunks.values().filter(|chunk| chunk.is_full()).count();
        let runtime_world = area.materialize(reader.dimension())?;
        println!(
            "Area centered at ({x}, {z}), radius {radius}: {} chunks ({full} full), {} missing; {block_sections} block sections, {palettes} palette entries; {} distinct states, {} biomes",
            area.chunks.len(),
            area.missing.len(),
            runtime_world.state_count(),
            runtime_world.biome_count()
        );
    }
    Ok(())
}

#[derive(Default)]
struct InputState {
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    jump: bool,
    sprint: toggle_key::ToggleKey,
    sneak: toggle_key::ToggleKey,
}

impl InputState {
    fn submit(&self, session: &mut LocalSession) {
        session.submit_command(PlayerCommand::Move {
            forward: i8::from(self.forward) as f32 - i8::from(self.back) as f32,
            strafe: i8::from(self.right) as f32 - i8::from(self.left) as f32,
            jump: self.jump,
        });
        session.submit_command(PlayerCommand::Sprint {
            pressed: self.sprint.down(),
            backward_pressed: self.back,
        });
        session.submit_command(PlayerCommand::Sneak(self.sneak.down()));
    }

    fn release_for_screen(&mut self, settings: menu_settings::VideoSettings) {
        self.forward = false;
        self.back = false;
        self.left = false;
        self.right = false;
        self.jump = false;
        self.sprint.release_for_screen(settings.toggle_sprint);
        self.sneak.release_for_screen(settings.toggle_sneak);
    }

    fn restore_after_screen(&mut self, settings: menu_settings::VideoSettings) {
        self.sprint.restore_after_screen(settings.toggle_sprint);
        self.sneak.restore_after_screen(settings.toggle_sneak);
    }
}

fn apply_game_video(
    ui: Option<&GameUi>,
    current: &mut menu_settings::VideoSettings,
    pacer: &mut frame_pacer::FramePacer,
    mut renderer: Option<&mut Renderer>,
) {
    let Some(next) = ui.map(GameUi::video_settings) else {
        return;
    };
    if next.vsync != current.vsync
        && let Some(renderer) = renderer.as_deref_mut()
    {
        renderer.set_vsync(next.vsync);
    }
    if next.max_fps != current.max_fps {
        *pacer = frame_pacer::FramePacer::new(next.max_fps, Instant::now());
    }
    if next.fov != current.fov
        && let Some(renderer) = renderer.as_deref_mut()
    {
        renderer.set_fov(next.fov);
    }
    if next.view_bobbing != current.view_bobbing
        && let Some(renderer) = renderer
    {
        renderer.set_view_bobbing(next.view_bobbing);
    }
    *current = next;
}

fn save_game_video(ui: Option<&mut GameUi>) -> bool {
    let Some(ui) = ui else { return true };
    if !ui.video_needs_save() {
        return true;
    }
    let result = menu_settings::save_video(ui.video_settings());
    let saved = result.is_ok();
    ui.video_save_result(result);
    saved
}

fn save_game_sound(ui: Option<&mut GameUi>) -> bool {
    let Some(ui) = ui else { return true };
    if !ui.sound_needs_save() {
        return true;
    }
    let result = menu_settings::save_sound(ui.sound_settings());
    let saved = result.is_ok();
    ui.sound_save_result(result);
    saved
}

fn finish_game_video(ui: Option<&mut GameUi>) {
    let Some(ui) = ui else { return };
    let mut errors = Vec::new();
    if !save_game_video(Some(&mut *ui)) {
        errors.push(
            ui.video_save_error()
                .unwrap_or("Could not save video settings")
                .to_owned(),
        );
    }
    if !save_game_sound(Some(&mut *ui)) {
        errors.push(
            ui.video_save_error()
                .unwrap_or("Could not save sound settings")
                .to_owned(),
        );
    }
    if !errors.is_empty() {
        rfd::MessageDialog::new()
            .set_title("Minecraft")
            .set_description(errors.join("\n"))
            .show();
    }
}

fn scroll_lines(delta: MouseScrollDelta) -> f64 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => f64::from(y),
        MouseScrollDelta::PixelDelta(position) => position.y / 20.0,
    }
}

struct App {
    atlas: TextureAtlas,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    session: LocalSession,
    visual_look: visual_look::VisualLook,
    input: InputState,
    mouse_bindings: toggle_key::MouseBindings,
    hotbar_scroll: mouse_scroll::HotbarScroll,
    captured: bool,
    ui: Option<GameUi>,
    audio: Option<audio::UiAudio>,
    audio_listener: Option<(glam::DVec3, f32)>,
    inventory_wait: Option<u64>,
    menu_open: bool,
    death_visible: bool,
    inventory_shift: bool,
    inventory_control: bool,
    last_update: Instant,
    occluded: bool,
    video: menu_settings::VideoSettings,
    quit_application: bool,
    frame_pacer: frame_pacer::FramePacer,
    inactivity: frame_pacer::InactivityTracker,
    focus_pause: focus_pause::FocusPause,
    #[cfg(debug_assertions)]
    stats_since: Instant,
    #[cfg(debug_assertions)]
    stats_tick: u64,
    #[cfg(debug_assertions)]
    frames: u64,
    #[cfg(debug_assertions)]
    total_frames: u64,
    #[cfg(debug_assertions)]
    smoke_test: bool,
    error: Option<anyhow::Error>,
}

impl App {
    fn new(
        atlas: TextureAtlas,
        smoke_test: bool,
        assets: Option<Arc<MenuAssets>>,
        content: SessionContent,
        video: menu_settings::VideoSettings,
    ) -> Self {
        let now = Instant::now();
        let _ = smoke_test;
        let mut session = LocalSession::demo();
        session.set_fov_effect_scale(video.fov_effect_scale.value() as f32);
        session.set_sprint_window(video.sprint_window);
        if let Some(recipes) = &content.recipes {
            session.set_crafting_recipes(Arc::clone(recipes));
        }
        if let Some(loot) = content.loot {
            session.set_block_loot(loot);
        }
        if let Some(enchantments) = content.enchantments {
            session.set_enchantments(enchantments);
        }
        for (kind, transforms) in [ToolKind::Axe, ToolKind::Shovel, ToolKind::Hoe]
            .into_iter()
            .zip(content.tools)
        {
            if let Some(transforms) = transforms {
                session.set_tool_transformers(kind, transforms);
            }
        }
        let mut ui = assets.map(GameUi::new);
        if let Some(ui) = &mut ui {
            ui.set_crafting_recipes(content.recipes);
            ui.set_video_settings(video);
        }
        Self {
            atlas,
            window: None,
            renderer: None,
            session,
            visual_look: visual_look::VisualLook::default(),
            input: InputState::default(),
            mouse_bindings: toggle_key::MouseBindings::default(),
            hotbar_scroll: mouse_scroll::HotbarScroll::default(),
            captured: false,
            ui,
            audio: None,
            audio_listener: None,
            inventory_wait: None,
            menu_open: false,
            death_visible: false,
            inventory_shift: false,
            inventory_control: false,
            last_update: now,
            occluded: false,
            video,
            quit_application: false,
            frame_pacer: frame_pacer::FramePacer::new(video.max_fps, now),
            inactivity: frame_pacer::InactivityTracker::new(now),
            focus_pause: focus_pause::FocusPause::default(),
            #[cfg(debug_assertions)]
            stats_since: now,
            #[cfg(debug_assertions)]
            stats_tick: 0,
            #[cfg(debug_assertions)]
            frames: 0,
            #[cfg(debug_assertions)]
            total_frames: 0,
            #[cfg(debug_assertions)]
            smoke_test,
            error: None,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn capture(&mut self, enabled: bool) {
        if !enabled {
            self.mouse_bindings.reset();
            self.session.submit_command(PlayerCommand::StopBreak);
            self.session.submit_command(PlayerCommand::StopUse);
        }
        if let Some(window) = &self.window {
            if enabled {
                match window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                {
                    Ok(()) => {
                        window.set_cursor_visible(false);
                        self.captured = true;
                    }
                    Err(error) => eprintln!("Could not capture the mouse: {error}"),
                }
            } else {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
                self.captured = false;
            }
        }
    }

    fn apply_video(&mut self) {
        let old_fov_effect = self.video.fov_effect_scale;
        let old_sprint_window = self.video.sprint_window;
        apply_game_video(
            self.ui.as_ref(),
            &mut self.video,
            &mut self.frame_pacer,
            self.renderer.as_mut(),
        );
        if self.video.fov_effect_scale != old_fov_effect {
            self.session
                .set_fov_effect_scale(self.video.fov_effect_scale.value() as f32);
        }
        if self.video.sprint_window != old_sprint_window {
            self.session.set_sprint_window(self.video.sprint_window);
        }
    }

    fn pause_action(&mut self, action: PauseAction, event_loop: &ActiveEventLoop) {
        self.apply_video();
        match action {
            PauseAction::SaveVideo => {
                save_game_video(self.ui.as_mut());
            }
            PauseAction::SaveSound => {
                save_game_sound(self.ui.as_mut());
            }
            PauseAction::Resume => {
                save_game_video(self.ui.as_mut());
                save_game_sound(self.ui.as_mut());
                if let Some(ui) = &mut self.ui {
                    ui.resume();
                }
                self.input.restore_after_screen(self.video);
                self.input.submit(&mut self.session);
                self.last_update = Instant::now();
                self.capture(true);
            }
            PauseAction::QuitToTitle => {
                finish_game_video(self.ui.as_mut());
                event_loop.exit();
            }
            PauseAction::None => {}
        }
    }

    fn death_action(&mut self, action: death_ui::DeathAction, event_loop: &ActiveEventLoop) {
        match action {
            death_ui::DeathAction::Respawn => self.session.submit_command(PlayerCommand::Respawn),
            death_ui::DeathAction::QuitToTitle => {
                self.pause_action(PauseAction::QuitToTitle, event_loop)
            }
            death_ui::DeathAction::None => {}
        }
    }

    fn key(&mut self, code: KeyCode, pressed: bool, repeat: bool, event_loop: &ActiveEventLoop) {
        let snapshot = self.session.snapshot();
        let screen_open = self.ui.is_none()
            || self.inventory_wait.is_some()
            || snapshot.inventory.is_open()
            || snapshot.death.is_some()
            || self.ui.as_ref().is_some_and(GameUi::paused);
        if self.focus_pause.key(code, pressed, screen_open) {
            self.video.pause_on_lost_focus = !self.video.pause_on_lost_focus;
            if let Some(ui) = &mut self.ui {
                ui.debug_focus_pause(self.video.pause_on_lost_focus, snapshot.tick);
                save_game_video(Some(ui));
            }
            return;
        }
        if snapshot.death.is_some() {
            if let Some(ui) = &mut self.ui {
                ui.sync_death(&snapshot);
                if pressed && !repeat {
                    let action = ui.death_key(code, self.inventory_shift);
                    self.death_action(action, event_loop);
                }
            }
            return;
        }
        if pressed
            && !repeat
            && self.ui.is_some()
            && ((code == KeyCode::KeyE && !self.ui.as_ref().is_some_and(GameUi::paused))
                || (code == KeyCode::Escape && snapshot.inventory.is_open()))
        {
            if self.inventory_wait.is_none() {
                self.inventory_wait = Some(snapshot.tick);
                if self
                    .ui
                    .as_mut()
                    .is_some_and(GameUi::inventory_pointer_cancel)
                {
                    self.session.submit_command(PlayerCommand::InventoryPointer(
                        InventoryPointerEvent::Cancel,
                    ));
                }
                self.session.submit_command(PlayerCommand::ToggleInventory);
                self.input.release_for_screen(self.video);
                self.input.submit(&mut self.session);
                self.capture(false);
            }
            return;
        }
        if self.inventory_wait.is_some() {
            return;
        }
        if self.session.snapshot().inventory.is_open() {
            if pressed
                && let Some(target) = inventory_swap_key(code)
                && let Some(slot) = self.ui.as_ref().and_then(GameUi::inventory_slot)
            {
                self.session
                    .submit_command(PlayerCommand::InventorySwap { slot, target });
                return;
            }
            if pressed
                && code == KeyCode::KeyQ
                && let Some(slot) = self.ui.as_ref().and_then(GameUi::inventory_slot)
            {
                self.session.submit_command(PlayerCommand::InventoryThrow {
                    slot,
                    all: self.inventory_control,
                });
            }
            return;
        }
        if self.ui.as_ref().is_some_and(GameUi::paused) {
            if pressed && (!repeat || matches!(code, KeyCode::ArrowLeft | KeyCode::ArrowRight)) {
                let action = self
                    .ui
                    .as_mut()
                    .unwrap()
                    .pause_key(code, self.inventory_shift);
                self.pause_action(action, event_loop);
            }
            return;
        }
        if code == KeyCode::Escape && pressed && !repeat {
            if let Some(ui) = &mut self.ui {
                let paused = ui.toggle_pause();
                self.input.release_for_screen(self.video);
                self.input.submit(&mut self.session);
                self.capture(!paused);
                self.last_update = Instant::now();
            } else {
                self.capture(false);
            }
            return;
        }
        if pressed && code == KeyCode::KeyQ && self.captured {
            self.session.submit_command(PlayerCommand::DropSelected {
                all: self.inventory_control,
            });
            return;
        }
        if pressed && !repeat && code == KeyCode::KeyF && self.captured {
            self.session.submit_command(PlayerCommand::SwapHands);
            return;
        }
        match code {
            KeyCode::KeyW => self.input.forward = pressed,
            KeyCode::KeyS => self.input.back = pressed,
            KeyCode::KeyA => self.input.left = pressed,
            KeyCode::KeyD => self.input.right = pressed,
            KeyCode::Space => self.input.jump = pressed,
            KeyCode::ControlLeft => {
                self.input
                    .sprint
                    .set(true, pressed, repeat, self.video.toggle_sprint)
            }
            KeyCode::ControlRight => {
                self.input
                    .sprint
                    .set(false, pressed, repeat, self.video.toggle_sprint)
            }
            KeyCode::ShiftLeft => {
                self.input
                    .sneak
                    .set(true, pressed, repeat, self.video.toggle_sneak)
            }
            KeyCode::ShiftRight => {
                self.input
                    .sneak
                    .set(false, pressed, repeat, self.video.toggle_sneak)
            }
            code if pressed && !repeat && hotbar_key(code).is_some() => self
                .session
                .submit_command(PlayerCommand::SelectSlot(hotbar_key(code).unwrap())),
            _ => return,
        }
        self.input.submit(&mut self.session);
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(ui) = &mut self.ui {
            ui.advance_video_settings(Instant::now());
        }
        self.apply_video();
        #[cfg(debug_assertions)]
        let now = Instant::now();
        let snapshot = self.session.snapshot();
        if self.death_visible != snapshot.death.is_some() {
            self.death_visible = snapshot.death.is_some();
            self.inventory_wait = None;
            if self.death_visible {
                self.input = InputState::default();
                self.capture(false);
            } else if !snapshot.inventory.is_open() && !self.ui.as_ref().is_some_and(GameUi::paused)
            {
                self.capture(true);
            }
        }
        if let Some(ui) = &mut self.ui {
            ui.sync_death(&snapshot);
        }
        if self.inventory_wait.is_some_and(|tick| snapshot.tick > tick) {
            self.inventory_wait = None;
        }
        if self.ui.is_some() && self.menu_open != snapshot.inventory.is_open() {
            self.menu_open = snapshot.inventory.is_open();
            if self.menu_open {
                self.input.release_for_screen(self.video);
                self.input.submit(&mut self.session);
                self.capture(false);
            } else if snapshot.death.is_none() && !self.ui.as_ref().is_some_and(GameUi::paused) {
                self.input.restore_after_screen(self.video);
                self.input.submit(&mut self.session);
                self.capture(true);
            }
        }
        if let (Some(ui), Some(renderer), Some(window)) =
            (&mut self.ui, &mut self.renderer, &self.window)
        {
            let size = window.inner_size();
            if let Err(error) = ui.draw(
                renderer,
                (size.width, size.height),
                &snapshot,
                self.session.partial_tick(),
            ) {
                self.fail(event_loop, error);
                return;
            }
        }
        let direction = self.visual_look.direction(&snapshot);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_view_yaw(self.visual_look.angles(&snapshot).0);
        }
        if let Some(renderer) = &mut self.renderer
            && let Err(error) = renderer.render(&snapshot, self.session.partial_tick(), direction)
        {
            self.fail(event_loop, error);
            #[cfg(debug_assertions)]
            return;
        }
        #[cfg(debug_assertions)]
        {
            self.frames += 1;
            self.total_frames += 1;
            if self.smoke_test && self.total_frames >= 3 {
                println!("Graphics smoke test passed: {} frames", self.total_frames);
                event_loop.exit();
                return;
            }
            let stats_time = now.saturating_duration_since(self.stats_since);
            if stats_time >= Duration::from_secs(1) {
                if let (Some(window), Some(renderer)) = (&self.window, &self.renderer) {
                    let fps = self.frames as f64 / stats_time.as_secs_f64();
                    let tps = (snapshot.tick - self.stats_tick) as f64 / stats_time.as_secs_f64();
                    window.set_title(&format!("Minecraft Native | {} | {:.0} FPS / {:.1} TPS | {:?} [1-9] | click to play, Esc releases mouse", renderer.backend_name, fps, tps, snapshot.selected_block));
                }
                self.frames = 0;
                self.stats_tick = snapshot.tick;
                self.stats_since = now;
            }
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Minecraft 26.3")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
            .with_min_inner_size(winit::dpi::LogicalSize::new(320, 240));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(event_loop, error.into());
                return;
            }
        };
        let mut renderer = match Renderer::new(
            Arc::clone(&window),
            &self.atlas,
            self.ui.as_ref().map(GameUi::breaking_textures),
            self.ui.as_ref().map(GameUi::item_textures),
            self.ui.as_ref().map(GameUi::experience_orb_texture),
            self.ui.as_ref().map(GameUi::item_glint_texture),
            self.video.vsync,
        ) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        renderer.set_fov(self.video.fov);
        renderer.set_view_bobbing(self.video.view_bobbing);
        #[cfg(debug_assertions)]
        println!("Graphics backend: {}", renderer.backend_name);
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.last_update = Instant::now();
        self.frame_pacer = frame_pacer::FramePacer::new(self.video.max_fps, self.last_update);
        self.inactivity = frame_pacer::InactivityTracker::new(self.last_update);
        #[cfg(debug_assertions)]
        {
            self.stats_since = self.last_update;
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self
            .window
            .as_ref()
            .is_none_or(|window| window.id() != window_id)
        {
            return;
        }
        if matches!(
            event,
            WindowEvent::KeyboardInput { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::CursorMoved { .. }
        ) {
            let now = Instant::now();
            if self.inactivity.input(now, self.video.inactivity_fps_limit) {
                self.frame_pacer.set_limit(self.video.max_fps, now);
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                finish_game_video(self.ui.as_mut());
                self.quit_application = true;
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size);
                }
            }
            WindowEvent::Occluded(occluded) => self.occluded = occluded,
            WindowEvent::ModifiersChanged(modifiers) => {
                self.inventory_shift = modifiers.state().shift_key();
                self.inventory_control = modifiers.state().control_key();
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(ui) = &mut self.ui {
                    ui.cursor_moved(position.x, position.y);
                    if let Some(slot) = ui.inventory_pointer_move() {
                        self.session.submit_command(PlayerCommand::InventoryPointer(
                            InventoryPointerEvent::Move { slot },
                        ));
                    }
                }
                self.apply_video();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = mouse_scroll::scaled_axes(mouse_scroll::event_axes(delta), self.video);
                if self.ui.as_ref().is_some_and(GameUi::paused) {
                    if let Some(ui) = &mut self.ui {
                        ui.pause_scroll(amount.1);
                    }
                } else {
                    let snapshot = self.session.snapshot();
                    if self.captured && snapshot.death.is_none() && !snapshot.inventory.is_open() {
                        if let Some(command) = self.hotbar_scroll.game_command(
                            amount,
                            snapshot.game_mode,
                            snapshot.inventory.selected(),
                        ) {
                            self.session.submit_command(command);
                        }
                    }
                }
            }
            WindowEvent::Focused(focused) => {
                self.focus_pause.focus_changed(focused, Instant::now());
                if focused {
                    if self.inventory_wait.is_none()
                        && !self.session.snapshot().inventory.is_open()
                        && !self.ui.as_ref().is_some_and(GameUi::paused)
                    {
                        self.input.restore_after_screen(self.video);
                        self.input.submit(&mut self.session);
                    }
                    return;
                }
                self.inventory_shift = false;
                self.inventory_control = false;
                self.input.release_for_screen(self.video);
                self.input.submit(&mut self.session);
                self.capture(false);
                if let Some(ui) = &mut self.ui {
                    if ui.inventory_pointer_cancel() {
                        self.session.submit_command(PlayerCommand::InventoryPointer(
                            InventoryPointerEvent::Cancel,
                        ));
                    }
                    ui.recipe_book_unfocus();
                    ui.pause_cancel();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if self.session.snapshot().death.is_none()
                        && self.session.snapshot().inventory.is_open()
                        && event.state == ElementState::Pressed
                    {
                        if code == KeyCode::Escape
                            && let Some(BookAction::Settings { open, filtering }) =
                                self.ui.as_mut().and_then(GameUi::recipe_book_escape)
                        {
                            self.session
                                .submit_command(PlayerCommand::RecipeBookSettings {
                                    open,
                                    filtering,
                                });
                            return;
                        }
                        if let Some(ui) = &mut self.ui
                            && ui.recipe_book_captures_text()
                            && code != KeyCode::Escape
                        {
                            if code == KeyCode::Backspace {
                                ui.recipe_book_backspace();
                            } else if !self.inventory_control
                                && let Some(text) = &event.text
                            {
                                ui.recipe_book_input(text);
                            }
                            return;
                        }
                    }
                    self.key(
                        code,
                        event.state == ElementState::Pressed,
                        event.repeat,
                        event_loop,
                    );
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button,
                ..
            } => {
                if self.session.snapshot().death.is_some() {
                    return;
                }
                if button == MouseButton::Left && self.ui.as_ref().is_some_and(GameUi::paused) {
                    if let Some(ui) = &mut self.ui {
                        ui.pause_release();
                    }
                } else if self.session.snapshot().inventory.is_open() {
                    let button = match button {
                        MouseButton::Left => Some(ClickButton::Primary),
                        MouseButton::Right => Some(ClickButton::Secondary),
                        _ => None,
                    };
                    if let Some(button) = button
                        && let Some(target) =
                            self.ui.as_mut().and_then(GameUi::inventory_pointer_release)
                    {
                        self.session.submit_command(PlayerCommand::InventoryPointer(
                            InventoryPointerEvent::Release {
                                target,
                                button,
                                shift: self.inventory_shift,
                            },
                        ));
                    }
                } else if self.captured && !self.ui.as_ref().is_some_and(GameUi::paused) {
                    match button {
                        MouseButton::Left => self.mouse_bindings.attack(
                            &mut self.session,
                            false,
                            self.video.toggle_attack,
                        ),
                        MouseButton::Right => self.mouse_bindings.use_item(
                            &mut self.session,
                            false,
                            self.video.toggle_use,
                        ),
                        _ => {}
                    }
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button,
                ..
            } => {
                let snapshot = self.session.snapshot();
                if snapshot.death.is_some() {
                    if button == MouseButton::Left
                        && let Some(ui) = &mut self.ui
                    {
                        ui.sync_death(&snapshot);
                        let action = ui.death_click();
                        self.death_action(action, event_loop);
                    }
                    return;
                }
                if self.inventory_wait.is_some() {
                    return;
                }
                if self.session.snapshot().inventory.is_open() {
                    if self
                        .ui
                        .as_ref()
                        .is_some_and(GameUi::inventory_pointer_active)
                    {
                        return;
                    }
                    if let Some(button) = match button {
                        MouseButton::Left => Some(ClickButton::Primary),
                        MouseButton::Right => Some(ClickButton::Secondary),
                        _ => None,
                    } {
                        let snapshot = self.session.snapshot();
                        let action = self
                            .ui
                            .as_mut()
                            .map(|ui| {
                                ui.recipe_book_click(
                                    button,
                                    self.inventory_shift,
                                    &snapshot.inventory,
                                )
                            })
                            .unwrap_or(BookAction::None);
                        match action {
                            BookAction::None => {}
                            BookAction::Consumed => return,
                            BookAction::Settings { open, filtering } => {
                                self.session
                                    .submit_command(PlayerCommand::RecipeBookSettings {
                                        open,
                                        filtering,
                                    });
                                return;
                            }
                            BookAction::Place {
                                recipe,
                                use_max,
                                close,
                                filtering,
                            } => {
                                self.session.submit_command(PlayerCommand::RecipeBookPlace {
                                    recipe,
                                    use_max,
                                });
                                if close {
                                    self.session.submit_command(
                                        PlayerCommand::RecipeBookSettings {
                                            open: false,
                                            filtering,
                                        },
                                    );
                                }
                                return;
                            }
                        }
                        if let Some(target) = self
                            .ui
                            .as_mut()
                            .and_then(|ui| ui.inventory_pointer_press(button))
                        {
                            self.session.submit_command(PlayerCommand::InventoryPointer(
                                InventoryPointerEvent::Press {
                                    target,
                                    button,
                                    shift: self.inventory_shift,
                                },
                            ));
                        }
                    }
                } else if self.ui.as_ref().is_some_and(GameUi::paused) {
                    if button == MouseButton::Left {
                        let action = self
                            .ui
                            .as_mut()
                            .map(GameUi::click)
                            .unwrap_or(PauseAction::None);
                        self.pause_action(action, event_loop);
                    }
                } else if !self.captured {
                    self.capture(true);
                } else {
                    match button {
                        MouseButton::Left => self.mouse_bindings.attack(
                            &mut self.session,
                            true,
                            self.video.toggle_attack,
                        ),
                        MouseButton::Middle => {
                            self.session.submit_command(PlayerCommand::PickBlock)
                        }
                        MouseButton::Right => {
                            self.mouse_bindings.use_item(
                                &mut self.session,
                                true,
                                self.video.toggle_use,
                            );
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if self.captured
            && let DeviceEvent::MouseMotion { delta } = event
        {
            let now = Instant::now();
            if self.inactivity.input(now, self.video.inactivity_fps_limit) {
                self.frame_pacer.set_limit(self.video.max_fps, now);
            }
            let (yaw, pitch) = mouse_look::look_delta(delta, self.video);
            self.visual_look.turn(&self.session.snapshot(), yaw, pitch);
            self.session
                .submit_command(PlayerCommand::Look { yaw, pitch });
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if self.ui.is_some()
            && self
                .focus_pause
                .should_pause(now, false, self.video.pause_on_lost_focus)
        {
            let snapshot = self.session.snapshot();
            let screen_open = self.ui.as_ref().is_some_and(GameUi::paused)
                || snapshot.inventory.is_open()
                || snapshot.death.is_some()
                || self.inventory_wait.is_some();
            if self
                .focus_pause
                .should_pause(now, screen_open, self.video.pause_on_lost_focus)
            {
                self.ui.as_mut().unwrap().toggle_pause();
                self.capture(false);
            }
        }
        if let Some(window) = &self.window {
            if !self.ui.as_ref().is_some_and(GameUi::paused) {
                self.session
                    .update(now.saturating_duration_since(self.last_update));
            }
            if let Some(audio) = &self.audio {
                let snapshot = self.session.snapshot();
                let eye = snapshot.camera.eye(self.session.partial_tick());
                let (yaw, _) = self.visual_look.angles(&snapshot);
                if self.audio_listener != Some((eye, yaw)) {
                    audio.set_listener(eye, yaw);
                    self.audio_listener = Some((eye, yaw));
                }
                for sound in self.session.take_pickup_sounds() {
                    audio.play_pickup(sound);
                }
            } else {
                self.session.take_pickup_sounds();
            }
            self.last_update = now;
            let size = window.inner_size();
            if self.occluded || size.width == 0 || size.height == 0 {
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(50)));
            } else {
                let fps = self.inactivity.limit(
                    now,
                    self.video.inactivity_fps_limit,
                    self.video.max_fps,
                    false,
                    false,
                );
                self.frame_pacer.set_limit(fps, now);
                if self.frame_pacer.due(now) {
                    window.request_redraw();
                }
                if let Some(deadline) = self.frame_pacer.deadline() {
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        deadline.min(now + Duration::from_millis(50)),
                    ));
                } else {
                    event_loop.set_control_flow(ControlFlow::Poll);
                }
            }
        }
    }
}

#[cfg(test)]
mod world_open_tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use native_persistence::nbt::{self, TagType};
    use std::io::Write;

    #[test]
    #[ignore = "requires locally installed Minecraft 26.3 assets"]
    fn saved_player_dimension_selects_the_matching_world_data() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let parent = tempfile::tempdir().unwrap();
        fn gzip(path: &std::path::Path, tag: &NamedTag) -> Vec<u8> {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&nbt::write(tag).unwrap()).unwrap();
            let bytes = encoder.finish().unwrap();
            std::fs::write(path, &bytes).unwrap();
            bytes
        }
        for (index, name, expected_type) in [
            (0, "minecraft:overworld", "minecraft:overworld"),
            (1, "minecraft:the_nether", "minecraft:the_nether"),
            (2, "minecraft:the_end", "minecraft:the_end"),
            (3, "example:custom/nested", "minecraft:the_nether"),
        ] {
            let root = parent.path().join(format!("world-{index}"));
            std::fs::create_dir_all(&root).unwrap();
            let level = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![(
                    "Data".into(),
                    Tag::Compound(vec![
                        ("DataVersion".into(), Tag::Int(5023)),
                        ("singleplayer_uuid".into(), Tag::IntArray(vec![0, 0, 0, 1])),
                    ]),
                )]),
            };
            let level_path = root.join("level.dat");
            let level_bytes = gzip(&level_path, &level);
            let player = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("Dimension".into(), Tag::String(name.into())),
                    (
                        "Pos".into(),
                        Tag::List {
                            element_type: TagType::Double,
                            values: vec![Tag::Double(8.5), Tag::Double(65.0), Tag::Double(8.5)],
                        },
                    ),
                    (
                        "Rotation".into(),
                        Tag::List {
                            element_type: TagType::Float,
                            values: vec![Tag::Float(0.0), Tag::Float(0.0)],
                        },
                    ),
                ]),
            };
            let player_path = root.join("players/data/00000000-0000-0000-0000-000000000001.dat");
            let player_bytes = gzip(&player_path, &player);
            let settings_path = root.join("data/minecraft/world_gen_settings.dat");
            let settings = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    (
                        "data".into(),
                        Tag::Compound(vec![(
                            "dimensions".into(),
                            Tag::Compound(vec![(
                                name.into(),
                                Tag::Compound(vec![
                                    ("type".into(), Tag::String(expected_type.into())),
                                    ("generator".into(), Tag::Compound(vec![])),
                                ]),
                            )]),
                        )]),
                    ),
                ]),
            };
            let settings_bytes = gzip(&settings_path, &settings);
            let chunk = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![
                    ("DataVersion".into(), Tag::Int(5023)),
                    ("xPos".into(), Tag::Int(0)),
                    ("zPos".into(), Tag::Int(0)),
                    ("Status".into(), Tag::String("minecraft:full".into())),
                    (
                        "sections".into(),
                        Tag::List {
                            element_type: TagType::Compound,
                            values: vec![Tag::Compound(vec![
                                ("Y".into(), Tag::Byte(0)),
                                (
                                    "block_states".into(),
                                    Tag::Compound(vec![(
                                        "palette".into(),
                                        Tag::List {
                                            element_type: TagType::String,
                                            values: vec![Tag::String("minecraft:stone".into())],
                                        },
                                    )]),
                                ),
                            ])],
                        },
                    ),
                ]),
            };
            let encoded = nbt::write(&chunk).unwrap();
            assert!(encoded.len() + 5 < 4096);
            let dimension = DimensionId::parse(name).unwrap();
            let region_path = root
                .join("dimensions")
                .join(&dimension.namespace)
                .join(&dimension.path)
                .join("region/r.0.0.mca");
            std::fs::create_dir_all(region_path.parent().unwrap()).unwrap();
            let mut region = vec![0u8; 3 * 4096];
            region[0..4].copy_from_slice(&((2u32 << 8) | 1).to_be_bytes());
            region[8192..8196].copy_from_slice(&((encoded.len() + 1) as u32).to_be_bytes());
            region[8196] = 3;
            region[8197..8197 + encoded.len()].copy_from_slice(&encoded);
            std::fs::write(&region_path, &region).unwrap();
            let prepared = prepare_existing_world(
                &root,
                &game,
                &[],
                ViewOptions {
                    dimension: None,
                    chunk: None,
                    radius: 1,
                    smoke_test: false,
                    block_report: None,
                    play: true,
                    video: menu_settings::VideoSettings::default(),
                },
                None,
            )
            .unwrap();
            let PreparedWorld::Play(prepared) = prepared else {
                panic!("expected play session")
            };
            let snapshot = prepared.session.snapshot();
            let loaded = snapshot.world.loaded().unwrap();
            assert_eq!(loaded.dimension, name);
            assert_eq!(loaded.dimension_type.as_deref(), Some(expected_type));
            assert_eq!(loaded.block(0, 0, 0).unwrap().name, "minecraft:stone");
            assert_eq!(snapshot.player.position.y, 65.0);
            assert_eq!(std::fs::read(level_path).unwrap(), level_bytes);
            assert_eq!(std::fs::read(player_path).unwrap(), player_bytes);
            assert_eq!(std::fs::read(settings_path).unwrap(), settings_bytes);
            assert_eq!(std::fs::read(region_path).unwrap(), region);
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod video_window_tests {
    use super::*;
    use winit::platform::windows::EventLoopBuilderExtWindows;

    #[test]
    #[ignore = "requires the local Minecraft 26.3 assets and a Windows GPU window"]
    fn focus_loss_opens_pause_screen_and_stops_ticks() {
        if !crate::window_test::isolated(
            "video_window_tests::focus_loss_opens_pause_screen_and_stops_ticks",
        ) {
            return;
        }
        struct Harness {
            app: App,
            lost_at: Option<Instant>,
            paused_tick: Option<u64>,
            checked_delay: bool,
        }
        impl ApplicationHandler for Harness {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.app.resumed(event_loop);
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                let redraw = matches!(event, WindowEvent::RedrawRequested);
                self.app.window_event(event_loop, id, event);
                if redraw && self.lost_at.is_none() {
                    self.app
                        .window_event(event_loop, id, WindowEvent::Focused(false));
                    self.lost_at = Some(Instant::now());
                    assert!(!self.app.ui.as_ref().unwrap().paused());
                    self.checked_delay = true;
                }
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.app.about_to_wait(event_loop);
                let Some(lost) = self.lost_at else { return };
                if self.app.ui.as_ref().unwrap().paused() && self.paused_tick.is_none() {
                    assert!(lost.elapsed() >= Duration::from_millis(490));
                    self.paused_tick = Some(self.app.session.snapshot().tick);
                }
                if lost.elapsed() >= Duration::from_millis(1100) {
                    assert!(self.checked_delay);
                    assert_eq!(self.app.session.snapshot().tick, self.paused_tick.unwrap());
                    event_loop.exit();
                }
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let mut harness = Harness {
            app: App::new(
                native_resources::load_atlas(&game).unwrap(),
                false,
                Some(Arc::new(MenuAssets::load(&game, &[]).unwrap())),
                SessionContent::default(),
                menu_settings::VideoSettings::default(),
            ),
            lost_at: None,
            paused_tick: None,
            checked_delay: false,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(harness.app.error.is_none(), "{:?}", harness.app.error);
        assert!(harness.paused_tick.is_some());
    }

    #[test]
    #[ignore = "requires the local Minecraft 26.3 assets and a Windows GPU window"]
    fn paused_game_applies_video_settings_without_advancing_ticks() {
        if !crate::window_test::isolated(
            "video_window_tests::paused_game_applies_video_settings_without_advancing_ticks",
        ) {
            return;
        }
        struct Harness {
            app: App,
            audio: crate::audio::AudioProbe,
            paused_since: Option<Instant>,
            paused_tick: u64,
            paused_frames: usize,
        }
        impl ApplicationHandler for Harness {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.app.resumed(event_loop);
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                let redraw = matches!(event, WindowEvent::RedrawRequested);
                self.app.window_event(event_loop, id, event);
                if !redraw {
                    return;
                }
                if self.paused_since.is_none() {
                    self.app.key(KeyCode::Escape, true, false, event_loop);
                    for key in [
                        KeyCode::Tab,
                        KeyCode::Tab,
                        KeyCode::Enter,
                        KeyCode::Tab,
                        KeyCode::ArrowRight,
                        KeyCode::Tab,
                        KeyCode::Tab,
                        KeyCode::Enter,
                    ] {
                        self.app.key(key, true, false, event_loop);
                    }
                    for key in [
                        KeyCode::Tab,
                        KeyCode::Enter,
                        KeyCode::ArrowRight,
                        KeyCode::Tab,
                        KeyCode::Enter,
                    ] {
                        self.app.key(key, true, false, event_loop);
                    }
                    assert_eq!(
                        self.app.video,
                        menu_settings::VideoSettings {
                            vsync: false,
                            max_fps: 130,
                            fov: 71,
                            view_bobbing: true,
                            inactivity_fps_limit: menu_settings::InactivityFpsLimit::Afk,
                            ..menu_settings::VideoSettings::default()
                        }
                    );
                    assert_eq!(
                        self.app
                            .renderer
                            .as_ref()
                            .unwrap()
                            .configured_present_mode(),
                        wgpu::PresentMode::AutoNoVsync
                    );
                    assert_eq!(self.app.renderer.as_ref().unwrap().configured_fov(), 71.0);
                    assert!(self.app.ui.as_ref().unwrap().paused());
                    // Exercise the menu without writing the user's settings during this test.
                    assert_eq!(
                        self.app
                            .ui
                            .as_mut()
                            .unwrap()
                            .pause_key(KeyCode::Escape, false),
                        crate::game_ui::PauseAction::SaveVideo
                    );
                    for key in [
                        KeyCode::Tab,
                        KeyCode::Tab,
                        KeyCode::Tab,
                        KeyCode::Tab,
                        KeyCode::Enter,
                        KeyCode::Tab,
                        KeyCode::Tab,
                        KeyCode::Enter,
                    ] {
                        self.app.key(key, true, false, event_loop);
                    }
                    assert!(!self.app.video.view_bobbing);
                    assert!(
                        !self
                            .app
                            .renderer
                            .as_ref()
                            .unwrap()
                            .configured_view_bobbing()
                    );
                    assert!(
                        self.audio.click().is_some(),
                        "pause buttons must queue a UI sound"
                    );
                    self.paused_tick = self.app.session.snapshot().tick;
                    self.paused_since = Some(Instant::now());
                } else {
                    self.paused_frames += 1;
                }
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.app.about_to_wait(event_loop);
                if self
                    .paused_since
                    .is_some_and(|start| start.elapsed() >= Duration::from_millis(700))
                {
                    assert_eq!(self.app.session.snapshot().tick, self.paused_tick);
                    event_loop.exit();
                }
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let atlas = native_resources::load_atlas(&game).unwrap();
        let assets = Arc::new(MenuAssets::load(&game, &[]).unwrap());
        let (audio, probe) = crate::audio::Audio::test_channel();
        let mut harness = Harness {
            app: App::new(
                atlas,
                false,
                Some(assets),
                SessionContent::default(),
                menu_settings::VideoSettings::default(),
            ),
            audio: probe,
            paused_since: None,
            paused_tick: 0,
            paused_frames: 0,
        };
        harness
            .app
            .ui
            .as_mut()
            .unwrap()
            .set_audio(Some(crate::audio::UiAudio::new(audio, game, Vec::new())));
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(
            harness.app.error.is_none(),
            "game rendering error: {:?}",
            harness.app.error
        );
        assert!(harness.paused_frames > 0);
        println!(
            "Paused for 700ms: {} rendered frames, no simulation ticks",
            harness.paused_frames
        );
    }

    #[test]
    #[ignore = "requires the local Minecraft 26.3 assets and a Windows GPU window"]
    fn capped_game_window_keeps_simulation_ticks_and_uses_selected_vsync() {
        if !crate::window_test::isolated(
            "video_window_tests::capped_game_window_keeps_simulation_ticks_and_uses_selected_vsync",
        ) {
            return;
        }
        struct Harness {
            app: App,
            start: Option<Instant>,
            frames: usize,
            ticks: u64,
        }
        impl ApplicationHandler for Harness {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                self.app.resumed(event_loop);
                let renderer = self.app.renderer.as_ref().expect("game renderer");
                assert_eq!(
                    renderer.configured_present_mode(),
                    wgpu::PresentMode::AutoNoVsync
                );
                self.start = Some(Instant::now());
            }
            fn window_event(
                &mut self,
                event_loop: &ActiveEventLoop,
                id: WindowId,
                event: WindowEvent,
            ) {
                if matches!(event, WindowEvent::RedrawRequested) {
                    self.frames += 1;
                }
                self.app.window_event(event_loop, id, event);
            }
            fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
                self.app.about_to_wait(event_loop);
                if self
                    .start
                    .is_some_and(|start| start.elapsed() >= Duration::from_secs(2))
                {
                    self.ticks = self.app.session.snapshot().tick;
                    event_loop.exit();
                }
            }
        }
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| native_resources::default_minecraft_dir().unwrap());
        let atlas = native_resources::load_atlas(&game).unwrap();
        let video = menu_settings::VideoSettings {
            vsync: false,
            max_fps: 10,
            fov: 70,
            view_bobbing: true,
            inactivity_fps_limit: menu_settings::InactivityFpsLimit::Afk,
            ..menu_settings::VideoSettings::default()
        };
        let mut harness = Harness {
            app: App::new(atlas, false, None, SessionContent::default(), video),
            start: None,
            frames: 0,
            ticks: 0,
        };
        let mut builder = EventLoop::builder();
        builder.with_any_thread(true);
        builder.build().unwrap().run_app(&mut harness).unwrap();
        assert!(
            harness.app.error.is_none(),
            "game rendering error: {:?}",
            harness.app.error
        );
        assert!(
            (12..=24).contains(&harness.frames),
            "frames={}",
            harness.frames
        );
        assert!(
            (37..=44).contains(&harness.ticks),
            "ticks={}",
            harness.ticks
        );
        println!(
            "Two-second window: {} frames, {} simulation ticks",
            harness.frames, harness.ticks
        );
    }
}
