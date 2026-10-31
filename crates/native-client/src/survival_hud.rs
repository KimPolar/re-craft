//! The basic 26.3 armor, health, food and air HUD, using the selected pack's sprites.

use native_core::random::LegacyRandom;
use native_session::{GameMode, survival::PlayerVitals};

use crate::{
    menu_assets::{GuiSprite, HudSprites},
    menu_draw::{Canvas, Rect},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HudDisplay {
    visible: bool,
    armor: i32,
    health: i32,
    max_health_bits: u32,
    absorption: i32,
    food: i32,
    hunger_effect: bool,
    empty_saturation: bool,
    hardcore: bool,
    air: Option<AirBubbles>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AirBubbles {
    full: i32,
    current: i32,
    empty: i32,
    popping: bool,
}

impl HudDisplay {
    pub fn new(vitals: PlayerVitals, mode: GameMode) -> Self {
        Self::with_air(vitals, mode, false)
    }

    pub fn with_air(vitals: PlayerVitals, mode: GameMode, eyes_in_water: bool) -> Self {
        let max_health = if vitals.max_health.is_finite() {
            vitals.max_health.clamp(1.0, 1024.0)
        } else {
            20.0
        };
        let health = if vitals.health.is_finite() {
            vitals.health.clamp(0.0, max_health).ceil() as i32
        } else {
            0
        };
        let absorption = if vitals.absorption.is_finite() {
            vitals.absorption.clamp(0.0, 2048.0).ceil() as i32
        } else {
            0
        };
        let visible = matches!(mode, GameMode::Survival | GameMode::Adventure);
        let supply = vitals.air.clamp(0, 300);
        let bubble = |offset: i32| (((supply + offset) * 10) as f32 / 300.0).ceil() as i32;
        let air = (visible && (eyes_in_water || supply < 300)).then(|| {
            let full = bubble(-2);
            let current = bubble(0);
            let delay = i32::from(supply != 0 && eyes_in_water);
            let popping = eyes_in_water && full != current;
            AirBubbles {
                full,
                current: if popping { current } else { 0 },
                // Java can compute -1 at full air; it draws the same as zero.
                empty: (10 - bubble(delay)).clamp(0, 10),
                popping,
            }
        });
        Self {
            visible,
            armor: 0,
            health,
            max_health_bits: max_health.max(health as f32).to_bits(),
            absorption,
            food: vitals.food,
            hunger_effect: false,
            empty_saturation: vitals.saturation <= 0.0,
            hardcore: vitals.hardcore,
            air,
        }
    }

    pub fn with_armor(mut self, armor: Option<native_session::ArmorValues>) -> Self {
        self.armor = armor.map_or(0, native_session::ArmorValues::display_value);
        self
    }

    pub fn with_hunger_effect(mut self, hunger_effect: bool) -> Self {
        self.hunger_effect = hunger_effect;
        self
    }

    pub fn animation_key(self, tick: u64) -> Option<u64> {
        if !self.visible {
            return None;
        }
        let low_health = self.health.saturating_add(self.absorption) <= 4;
        let hunger_period = i64::from(self.food) * 3 + 1;
        let hunger_moves = self.empty_saturation && (tick as i32 as i64) % hunger_period == 0;
        let empty_air_moves = self.air.is_some_and(|air| air.empty == 10);
        (low_health || hunger_moves || empty_air_moves).then_some(tick)
    }
}

impl Default for HudDisplay {
    fn default() -> Self {
        Self::new(PlayerVitals::default(), GameMode::Survival)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Sprite {
    ArmorEmpty,
    ArmorHalf,
    ArmorFull,
    HeartContainer,
    HeartContainerHardcore,
    HeartFull,
    HeartHalf,
    HeartHardcoreFull,
    HeartHardcoreHalf,
    AbsorptionFull,
    AbsorptionHalf,
    AbsorptionHardcoreFull,
    AbsorptionHardcoreHalf,
    FoodEmpty,
    FoodFull,
    FoodHalf,
    FoodEmptyHunger,
    FoodFullHunger,
    FoodHalfHunger,
    Air,
    AirBursting,
    AirEmpty,
}

impl Sprite {
    fn image(self, sprites: &HudSprites) -> &GuiSprite {
        match self {
            Self::ArmorEmpty => &sprites.armor_empty,
            Self::ArmorHalf => &sprites.armor_half,
            Self::ArmorFull => &sprites.armor_full,
            Self::HeartContainer => &sprites.heart_container,
            Self::HeartContainerHardcore => &sprites.heart_container_hardcore,
            Self::HeartFull => &sprites.heart_full,
            Self::HeartHalf => &sprites.heart_half,
            Self::HeartHardcoreFull => &sprites.heart_hardcore_full,
            Self::HeartHardcoreHalf => &sprites.heart_hardcore_half,
            Self::AbsorptionFull => &sprites.absorption_full,
            Self::AbsorptionHalf => &sprites.absorption_half,
            Self::AbsorptionHardcoreFull => &sprites.absorption_hardcore_full,
            Self::AbsorptionHardcoreHalf => &sprites.absorption_hardcore_half,
            Self::FoodEmpty => &sprites.food_empty,
            Self::FoodFull => &sprites.food_full,
            Self::FoodHalf => &sprites.food_half,
            Self::FoodEmptyHunger => &sprites.food_empty_hunger,
            Self::FoodFullHunger => &sprites.food_full_hunger,
            Self::FoodHalfHunger => &sprites.food_half_hunger,
            Self::Air => &sprites.air,
            Self::AirBursting => &sprites.air_bursting,
            Self::AirEmpty => &sprites.air_empty,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Placement {
    sprite: Sprite,
    x: i32,
    y: i32,
}

fn placements(hud: HudDisplay, width: i32, height: i32, tick: u64) -> Vec<Placement> {
    if !hud.visible {
        return Vec::new();
    }
    let max_health = f32::from_bits(hud.max_health_bits);
    let hearts = (max_health / 2.0).ceil() as i32;
    let absorption_hearts = (hud.absorption + 1) / 2;
    let rows = ((max_health + hud.absorption as f32) / 20.0).ceil() as i32;
    let row_step = (10 - (rows - 2)).max(3);
    let left = width / 2 - 91;
    let right = width / 2 + 91;
    let top = height - 39;
    let mut random = LegacyRandom::new(i64::from((tick as i32).wrapping_mul(312_871)));
    let mut result = Vec::with_capacity(((hearts + absorption_hearts) * 2 + 30) as usize);
    if hud.armor > 0 {
        let y = top - (rows - 1) * row_step - 10;
        for index in 0..10 {
            let units = index * 2 + 1;
            let sprite = if units < hud.armor {
                Sprite::ArmorFull
            } else if units == hud.armor {
                Sprite::ArmorHalf
            } else {
                Sprite::ArmorEmpty
            };
            result.push(Placement {
                sprite,
                x: left + index * 8,
                y,
            });
        }
    }
    for index in (0..hearts + absorption_hearts).rev() {
        let x = left + (index % 10) * 8;
        let mut y = top - (index / 10) * row_step;
        if hud.health.saturating_add(hud.absorption) <= 4 {
            y += random.next_int_bound(2).unwrap();
        }
        result.push(Placement {
            sprite: if hud.hardcore {
                Sprite::HeartContainerHardcore
            } else {
                Sprite::HeartContainer
            },
            x,
            y,
        });
        let units = index * 2;
        if index >= hearts {
            let amount = units - hearts * 2;
            if amount < hud.absorption {
                let half = amount + 1 == hud.absorption;
                let sprite = match (hud.hardcore, half) {
                    (false, false) => Sprite::AbsorptionFull,
                    (false, true) => Sprite::AbsorptionHalf,
                    (true, false) => Sprite::AbsorptionHardcoreFull,
                    (true, true) => Sprite::AbsorptionHardcoreHalf,
                };
                result.push(Placement { sprite, x, y });
            }
        }
        if units < hud.health {
            let half = units + 1 == hud.health;
            let sprite = match (hud.hardcore, half) {
                (false, false) => Sprite::HeartFull,
                (false, true) => Sprite::HeartHalf,
                (true, false) => Sprite::HeartHardcoreFull,
                (true, true) => Sprite::HeartHardcoreHalf,
            };
            result.push(Placement { sprite, x, y });
        }
    }
    let jitter_period = i64::from(hud.food) * 3 + 1;
    let jitter_food = hud.empty_saturation && (tick as i32 as i64) % jitter_period == 0;
    for index in 0..10 {
        let x = right - index * 8 - 9;
        let y = top
            + if jitter_food {
                random.next_int_bound(3).unwrap() - 1
            } else {
                0
            };
        result.push(Placement {
            sprite: if hud.hunger_effect {
                Sprite::FoodEmptyHunger
            } else {
                Sprite::FoodEmpty
            },
            x,
            y,
        });
        if index * 2 + 1 < hud.food {
            result.push(Placement {
                sprite: if hud.hunger_effect {
                    Sprite::FoodFullHunger
                } else {
                    Sprite::FoodFull
                },
                x,
                y,
            });
        } else if index * 2 + 1 == hud.food {
            result.push(Placement {
                sprite: if hud.hunger_effect {
                    Sprite::FoodHalfHunger
                } else {
                    Sprite::FoodHalf
                },
                x,
                y,
            });
        }
    }
    if let Some(air) = hud.air {
        for index in 1..=10 {
            let x = right - (index - 1) * 8 - 9;
            let mut y = top - 10;
            let sprite = if index <= air.full {
                Some(Sprite::Air)
            } else if air.popping && index == air.current {
                Some(Sprite::AirBursting)
            } else if index > 10 - air.empty {
                if air.empty == 10 && (tick as i32) % 2 == 0 {
                    y += random.next_int_bound(2).unwrap();
                }
                Some(Sprite::AirEmpty)
            } else {
                None
            };
            if let Some(sprite) = sprite {
                result.push(Placement { sprite, x, y });
            }
        }
    }
    result
}

pub fn draw(canvas: &mut Canvas, sprites: &HudSprites, hud: HudDisplay, tick: u64) {
    for placement in placements(hud, canvas.width, canvas.height, tick) {
        let sprite = placement.sprite.image(sprites);
        canvas.gui_sprite(
            &sprite.image,
            Rect {
                x: placement.x,
                y: placement.y,
                w: 9,
                h: 9,
            },
            sprite.scaling,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        ffi::OsString,
        path::{Path, PathBuf},
        process::Command,
    };

    fn name(sprite: Sprite) -> &'static str {
        match sprite {
            Sprite::ArmorEmpty => "hud/armor_empty",
            Sprite::ArmorHalf => "hud/armor_half",
            Sprite::ArmorFull => "hud/armor_full",
            Sprite::HeartContainer => "hud/heart/container",
            Sprite::HeartContainerHardcore => "hud/heart/container_hardcore",
            Sprite::HeartFull => "hud/heart/full",
            Sprite::HeartHalf => "hud/heart/half",
            Sprite::HeartHardcoreFull => "hud/heart/hardcore_full",
            Sprite::HeartHardcoreHalf => "hud/heart/hardcore_half",
            Sprite::AbsorptionFull => "hud/heart/absorbing_full",
            Sprite::AbsorptionHalf => "hud/heart/absorbing_half",
            Sprite::AbsorptionHardcoreFull => "hud/heart/absorbing_hardcore_full",
            Sprite::AbsorptionHardcoreHalf => "hud/heart/absorbing_hardcore_half",
            Sprite::FoodEmpty => "hud/food_empty",
            Sprite::FoodFull => "hud/food_full",
            Sprite::FoodHalf => "hud/food_half",
            Sprite::FoodEmptyHunger => "hud/food_empty_hunger",
            Sprite::FoodFullHunger => "hud/food_full_hunger",
            Sprite::FoodHalfHunger => "hud/food_half_hunger",
            Sprite::Air => "hud/air",
            Sprite::AirBursting => "hud/air_bursting",
            Sprite::AirEmpty => "hud/air_empty",
        }
    }

    #[test]
    fn visibility_and_basic_heart_and_food_placement() {
        let display = HudDisplay::default();
        let result = placements(display, 320, 240, 0);
        assert_eq!(result.len(), 40);
        assert_eq!(
            result.first(),
            Some(&Placement {
                sprite: Sprite::HeartContainer,
                x: 141,
                y: 201
            })
        );
        assert_eq!(
            result.last(),
            Some(&Placement {
                sprite: Sprite::FoodFull,
                x: 170,
                y: 201
            })
        );
        for mode in [GameMode::Creative, GameMode::Spectator] {
            assert!(
                placements(HudDisplay::new(PlayerVitals::default(), mode), 320, 240, 0).is_empty()
            );
        }
        assert_eq!(display.animation_key(0), None);
        assert_eq!(
            HudDisplay::new(
                PlayerVitals {
                    saturation: 0.0,
                    ..PlayerVitals::default()
                },
                GameMode::Survival
            )
            .animation_key(61),
            Some(61)
        );
    }

    const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import com.google.gson.*;
import com.mojang.renderpearl.api.pipeline.RenderPipeline;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.Hud;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.Holder;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.food.FoodData;
import net.minecraft.world.Difficulty;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.GameType;

public class SurvivalHudOracle {
    static final class TestLevel extends ClientLevel {
        ClientLevel.ClientLevelData data;
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public ClientLevel.ClientLevelData getLevelData() { return data; }
    }
    static final class TestPlayer extends Player {
        TestLevel testLevel;
        FoodData food;
        int air; boolean eyes; boolean hunger; int armor;
        TestPlayer() { super(null, null); }
        @Override public Level level() { return testLevel; }
        @Override public FoodData getFoodData() { return food; }
        @Override public boolean hasEffect(Holder<MobEffect> effect) { return hunger && effect == net.minecraft.world.effect.MobEffects.HUNGER; }
        @Override public boolean isFullyFrozen() { return false; }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public int getAirSupply() { return air; }
        @Override public int getArmorValue() { return armor; }
        @Override public boolean isEyeInFluid(net.minecraft.tags.TagKey<net.minecraft.world.level.material.Fluid> tag) { return eyes; }
        @Override public void playSound(net.minecraft.sounds.SoundEvent sound, float volume, float pitch) { }
    }
    static final class Capture extends GuiGraphicsExtractor {
        JsonArray draw;
        Capture() { super(null, null, 0, 0); }
        @Override public void blitSprite(RenderPipeline pipeline, Identifier id, int x, int y, int w, int h) {
            JsonArray e = new JsonArray(); e.add(id.getPath()); e.add(x); e.add(y); e.add(w); e.add(h); draw.add(e);
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var level = (TestLevel) unsafe.allocateInstance(TestLevel.class);
        var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class); player.testLevel = level; player.food = new FoodData();
        var capture = (Capture) unsafe.allocateInstance(Capture.class);
        var hud = (Hud) unsafe.allocateInstance(Hud.class);
        Field rng = Hud.class.getDeclaredField("random"); rng.setAccessible(true); rng.set(hud, RandomSource.create(0));
        Field ticks = Hud.class.getDeclaredField("tickCount"); ticks.setAccessible(true);
        var hearts = Hud.class.getDeclaredMethod("extractHearts", GuiGraphicsExtractor.class, Player.class, int.class, int.class, int.class, int.class, float.class, int.class, int.class, int.class, boolean.class); hearts.setAccessible(true);
        var armor = Hud.class.getDeclaredMethod("extractArmor", GuiGraphicsExtractor.class, Player.class, int.class, int.class, int.class, int.class); armor.setAccessible(true);
        var food = Hud.class.getDeclaredMethod("extractFood", GuiGraphicsExtractor.class, Player.class, int.class, int.class); food.setAccessible(true);
        var bubbles = Hud.class.getDeclaredMethod("extractAirBubbles", GuiGraphicsExtractor.class, Player.class, int.class, int.class, int.class); bubbles.setAccessible(true);
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var result = new JsonArray();
        for (var entry : cases) {
            var c = entry.getAsJsonObject();
            boolean hardcore = c.get("hardcore").getAsBoolean();
            level.data = new ClientLevel.ClientLevelData(Difficulty.NORMAL, hardcore, false);
            player.food.setFoodLevel(c.get("food").getAsInt()); player.food.setSaturation(c.get("saturation").getAsFloat());
            player.air = c.has("air") ? c.get("air").getAsInt() : 300;
            player.armor = c.has("armor") ? c.get("armor").getAsInt() : 0;
            player.eyes = c.has("eyes") && c.get("eyes").getAsBoolean();
            player.hunger = c.has("hunger") && c.get("hunger").getAsBoolean();
            int tick = c.get("tick").getAsInt(); ticks.setInt(hud, tick); ((RandomSource) rng.get(hud)).setSeed((long)(tick * 312871));
            int health = (int)Math.ceil(c.get("health").getAsFloat());
            int absorb = (int)Math.ceil(c.get("absorption").getAsFloat());
            float maximum = Math.max(c.get("max_health").getAsFloat(), (float)health);
            int rows = (int)Math.ceil((maximum + absorb) / 20.0f);
            int step = Math.max(10 - (rows - 2), 3);
            capture.draw = new JsonArray();
            armor.invoke(null, capture, player, 201, rows, step, 69);
            hearts.invoke(hud, capture, player, 69, 201, step, -1, maximum, health, health, absorb, false);
            food.invoke(hud, capture, player, 201, 251);
            bubbles.invoke(hud, capture, player, 0, 181, 251);
            result.add(capture.draw);
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(result));
    }
}
"#;

    fn java_bin() -> PathBuf {
        if let Some(path) =
            std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
        {
            return PathBuf::from(path).join("bin");
        }
        PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME to Java 25 JDK"))
            .join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
    }

    fn classpath(game: &Path, classes: &Path) -> OsString {
        let version = game.join("versions/26.3");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(version.join("26.3.json")).unwrap()).unwrap();
        let mut paths = vec![version.join("26.3.jar"), classes.to_path_buf()];
        for library in manifest["libraries"].as_array().unwrap() {
            if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
                let path = game.join("libraries").join(path);
                if path.is_file() {
                    paths.push(path);
                }
            }
        }
        std::env::join_paths(paths).unwrap()
    }

    fn run(command: &mut Command) {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "HUD oracle failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
    fn health_and_food_sprites_match_vanilla_26_3() {
        let game = std::env::var_os("MINECRAFT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft")
            });
        let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
        let mut cases = Vec::new();
        for health in [1.0, 3.0, 4.0, 5.0, 9.5, 19.0, 20.0] {
            for absorption in [0.0, 1.0, 4.0] {
                for food in [0, 1, 6, 7, 13, 20] {
                    for hardcore in [false, true] {
                        for tick in [0, 1, 21, 62] {
                            let max_health = if tick == 62 { 30.0 } else { 20.0 };
                            let saturation = if tick % 2 == 0 { 0.0 } else { 5.0 };
                            cases.push(json!({"health":health,"max_health":max_health,"absorption":absorption,"food":food,"saturation":saturation,"hardcore":hardcore,"tick":tick}));
                        }
                    }
                }
            }
        }
        for air in -20..=320 {
            for eyes in [false, true] {
                for tick in 0..4 {
                    let health = if tick == 0 { 3.0 } else { 20.0 };
                    let food = if tick == 1 { 0 } else { 20 };
                    let saturation = if tick == 1 { 0.0 } else { 5.0 };
                    cases.push(json!({"health":health,"max_health":20.0,"absorption":0.0,"food":food,"saturation":saturation,"hardcore":false,"tick":tick,"air":air,"eyes":eyes}));
                }
            }
        }
        for armor in 0..=30 {
            for (health, maximum, absorption) in [
                (20.0, 20.0, 0.0),
                (3.0, 20.0, 1.0),
                (20.0, 40.0, 0.0),
                (20.0, 100.0, 4.0),
                (20.0, 200.0, 30.0),
            ] {
                cases.push(json!({"armor":armor,"health":health,"max_health":maximum,"absorption":absorption,"food":20,"saturation":5.0,"hardcore":false,"tick":1}));
            }
        }
        for food in 0..=20 {
            for tick in [0, 1, 21, 62] {
                cases.push(json!({"health":3,"max_health":20.0,"absorption":0,"food":food,"saturation":0.0,"hardcore":false,"tick":tick,"hunger":true}));
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("SurvivalHudOracle.java");
        let input = temp.path().join("input.json");
        let output = temp.path().join("output.json");
        std::fs::write(&source, ORACLE).unwrap();
        std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
        let java = java_bin();
        let cp = classpath(&game, temp.path());
        run(Command::new(java.join("javac.exe"))
            .arg("-encoding")
            .arg("UTF-8")
            .arg("-classpath")
            .arg(&cp)
            .arg(&source));
        run(Command::new(java.join("java.exe"))
            .arg("-classpath")
            .arg(&cp)
            .arg("SurvivalHudOracle")
            .arg(&input)
            .arg(&output));
        let results: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        for (i, (case, expected)) in cases.iter().zip(results.as_array().unwrap()).enumerate() {
            let vitals = PlayerVitals {
                health: case["health"].as_f64().unwrap() as f32,
                max_health: case["max_health"].as_f64().unwrap() as f32,
                absorption: case["absorption"].as_f64().unwrap() as f32,
                food: case["food"].as_i64().unwrap() as i32,
                saturation: case["saturation"].as_f64().unwrap() as f32,
                hardcore: case["hardcore"].as_bool().unwrap(),
                air: case["air"].as_i64().unwrap_or(300) as i32,
                ..PlayerVitals::default()
            };
            let actual: Vec<Value> = placements(
                HudDisplay::with_air(
                    vitals,
                    GameMode::Survival,
                    case["eyes"].as_bool().unwrap_or(false),
                )
                .with_armor(Some(native_session::ArmorValues {
                    armor: case["armor"].as_f64().unwrap_or(0.0),
                    toughness: 0.0,
                }))
                .with_hunger_effect(case["hunger"].as_bool().unwrap_or(false)),
                320,
                240,
                case["tick"].as_u64().unwrap(),
            )
            .iter()
            .map(|p| json!([name(p.sprite), p.x, p.y, 9, 9]))
            .collect();
            assert_eq!(&Value::Array(actual), expected, "case {i}: {case}");
        }
        println!(
            "Compared {} armor, health, food and air HUD scenarios with Minecraft 26.3",
            cases.len()
        );
    }
}
