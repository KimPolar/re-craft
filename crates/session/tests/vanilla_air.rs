//! Compare ordinary player breathing with Java 26.3's real LivingEntity.baseTick.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use native_session::{GameMode, survival::PlayerVitals};
use serde_json::{Value, json};

const ORACLE: &str = r#"
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.*;
import net.minecraft.core.*;
import net.minecraft.core.Holder;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.TagKey;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.effect.MobEffects;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.phys.*;
import net.minecraft.nbt.NbtIo;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.entity.player.Player;

public class AirOracle {
    static class TestLevel extends ServerLevel {
        boolean bubble; WorldBorder border;
        TestLevel() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public BlockState getBlockState(BlockPos pos) { return (bubble ? Blocks.BUBBLE_COLUMN : Blocks.AIR).defaultBlockState(); }
        @Override public WorldBorder getWorldBorder() { return border; }
        @Override public void broadcastEntityEvent(Entity entity, byte event) { }
    }
    static class TestDamage extends DamageSources {
        TestDamage() { super(null); }
        @Override public DamageSource drown() { return null; }
    }
    static class TestPlayer extends ServerPlayer {
        TestLevel world; TestDamage damage; Abilities abilities;
        int air; float health, absorption; boolean eyes;
        Map<Holder<MobEffect>, MobEffectInstance> effects = new HashMap<>();
        AttributeInstance oxygen;
        TestPlayer() { super(null, null, null, null); }
        @Override public ServerLevel level() { return world; }
        @Override public ItemStack getItemBySlot(EquipmentSlot slot) { return ItemStack.EMPTY; }
        @Override protected void computeSpeed() { }
        @Override protected void handlePortal() { }
        @Override public boolean canSpawnSprintParticle() { return false; }
        @Override protected boolean updateFluidInteraction() { return eyes; }
        @Override public void updateSwimming() { }
        @Override public boolean isInLava() { return false; }
        @Override public void checkBelowWorld() { }
        @Override public void setSharedFlagOnFire(boolean value) { }
        @Override public boolean isPassenger() { return false; }
        @Override public boolean isInWall() { return false; }
        @Override public boolean isAlive() { return health > 0; }
        @Override public boolean isDeadOrDying() { return false; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return eyes; }
        @Override public boolean canBreatheUnderwater() { return false; }
        @Override public boolean hasEffect(Holder<MobEffect> effect) { return effects.containsKey(effect); }
        @Override public AttributeInstance getAttribute(Holder<Attribute> attribute) { return attribute == Attributes.OXYGEN_BONUS ? oxygen : null; }
        @Override public int getAirSupply() { return air; }
        @Override public void setAirSupply(int value) { air = value; }
        @Override public Abilities getAbilities() { return abilities; }
        @Override public DamageSources damageSources() { return damage; }
        @Override public double getEyeY() { return 1.5; }
        @Override public BlockPos blockPosition() { return BlockPos.ZERO; }
        @Override protected void onChangedBlock(ServerLevel level, BlockPos pos) { }
        @Override protected void tickEffects() {
            var it = effects.values().iterator();
            while (it.hasNext()) if (!it.next().tickServer(world, this, () -> { })) it.remove();
        }
        @Override public boolean hurtServer(ServerLevel level, DamageSource source, float amount) {
            float absorbed = Math.min(absorption, amount); absorption -= absorbed;
            health = Math.max(0, health - (amount - absorbed)); return true;
        }
    }
    static void setField(Class<?> type, Object target, String name, Object value) throws Exception {
        Field field = type.getDeclaredField(name); field.setAccessible(true); field.set(target, value);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) f.get(null);
        var world = (TestLevel) unsafe.allocateInstance(TestLevel.class); world.border = new WorldBorder();
        var player = (TestPlayer) unsafe.allocateInstance(TestPlayer.class);
        player.world = world; player.damage = (TestDamage) unsafe.allocateInstance(TestDamage.class);
        player.abilities = new Abilities();
        player.effects = new HashMap<>();
        var lookup = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        setField(Entity.class, player, "position", Vec3.ZERO);
        setField(LivingEntity.class, player, "swingState", new LivingEntity.SwingState());
        player.setBoundingBox(new AABB(0, 0, 0, 0.6, 1.8, 0.6));
        var cases = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        var result = new JsonArray();
        int index = 0;
        for (var item : cases) {
            var c = item.getAsJsonObject();
            player.air = c.get("air").getAsInt(); player.health = c.get("health").getAsFloat();
            player.absorption = c.get("absorption").getAsFloat();
            player.abilities.invulnerable = c.get("invulnerable").getAsBoolean();
            player.effects.clear(); player.oxygen = null;
            if (args.length > 2) {
                var tag = NbtIo.read(Path.of(args[2], index + ".nbt"));
                var input = TagValueInput.create(ProblemReporter.DISCARDING, lookup, tag);
                for (var effect : input.read("active_effects", MobEffectInstance.CODEC.listOf()).orElse(List.of()))
                    player.effects.put(effect.getEffect(), effect);
                var attributes = new AttributeMap(Player.createAttributes().build());
                input.read("attributes", AttributeInstance.Packed.LIST_CODEC).ifPresent(attributes::apply);
                player.oxygen = attributes.getInstance(Attributes.OXYGEN_BONUS);
                setField(Entity.class, player, "random", new LegacyRandomSource(c.get("seed").getAsLong()));
            }
            var ticks = new JsonArray();
            for (int t = 0; t < 400; t++) {
                int phase = c.get("pattern").getAsInt();
                player.eyes = phase == 0 || t < 340 || (t >= 360 && t < 380);
                world.bubble = phase != 0 && t >= 320 && t < 340;
                player.baseTick();
                JsonArray tick = new JsonArray(); tick.add(player.air); tick.add(player.health); tick.add(player.absorption);
                for (var effect : List.of(MobEffects.WATER_BREATHING, MobEffects.CONDUIT_POWER, MobEffects.BREATH_OF_THE_NAUTILUS))
                    tick.add(player.effects.containsKey(effect) ? player.effects.get(effect).getDuration() : -99999);
                for (var effect : List.of(MobEffects.WATER_BREATHING, MobEffects.CONDUIT_POWER, MobEffects.BREATH_OF_THE_NAUTILUS)) {
                    var active = player.effects.get(effect);
                    JsonArray details = new JsonArray();
                    if (active != null) {
                        details.add(active.getAmplifier()); details.add(active.isAmbient());
                        details.add(active.isVisible()); details.add(active.showIcon());
                    }
                    tick.add(details);
                }
                ticks.add(tick);
            }
            result.add(ticks);
            index++;
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
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap())
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
        "Java air oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn ordinary_breathing_matches_vanilla_base_tick_with_mocked_damage() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut cases = Vec::new();
    for air in [-20, -19, 0, 1, 150, 299, 300, 301] {
        for invulnerable in [false, true] {
            for absorption in [0.0, 1.0, 3.0] {
                for pattern in 0..2 {
                    cases.push(json!({"air":air,"health":20.0,"absorption":absorption,"invulnerable":invulnerable,"pattern":pattern}));
                }
            }
        }
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("AirOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("AirOracle")
        .arg(&input)
        .arg(&output));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let mode = if case["invulnerable"].as_bool().unwrap() {
            GameMode::Creative
        } else {
            GameMode::Survival
        };
        let mut vitals = PlayerVitals {
            air: case["air"].as_i64().unwrap() as i32,
            health: 20.0,
            absorption: case["absorption"].as_f64().unwrap() as f32,
            ..Default::default()
        };
        for tick in 0..400 {
            let pattern = case["pattern"].as_i64().unwrap();
            let eyes = pattern == 0 || tick < 340 || (360..380).contains(&tick);
            let bubble = pattern != 0 && (320..340).contains(&tick);
            vitals.tick_air(eyes, bubble, mode);
            let entry = &expected[index][tick];
            assert_eq!(
                vitals.air,
                entry[0].as_i64().unwrap() as i32,
                "case {index} tick {tick}"
            );
            assert_eq!(
                vitals.health,
                entry[1].as_f64().unwrap() as f32,
                "case {index} tick {tick}"
            );
            assert_eq!(
                vitals.absorption,
                entry[2].as_f64().unwrap() as f32,
                "case {index} tick {tick}"
            );
        }
    }
    println!(
        "Compared {} breathing ticks with Minecraft 26.3",
        cases.len() * 400
    );
}

fn saved_effect(name: &str, durations: &[i32]) -> native_persistence::nbt::Tag {
    use native_persistence::nbt::Tag;
    fn details(durations: &[i32]) -> Tag {
        let mut fields = vec![("duration".into(), Tag::Int(durations[0]))];
        if durations.len() > 1 {
            fields.push(("hidden_effect".into(), details(&durations[1..])));
        }
        Tag::Compound(fields)
    }
    let Tag::Compound(mut fields) = details(durations) else {
        unreachable!()
    };
    fields.push(("id".into(), Tag::String(name.into())));
    Tag::Compound(fields)
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn saved_effects_and_oxygen_bonus_match_vanilla_ticks() {
    use native_core::random::LegacyRandom;
    use native_persistence::nbt::{NamedTag, Tag, TagType};
    use native_session::player_data::{
        advance_saved_effects, read_saved_breathing_effects, read_saved_oxygen_bonus,
    };

    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let effects = [
        vec![],
        vec![saved_effect("minecraft:water_breathing", &[1])],
        vec![saved_effect("minecraft:conduit_power", &[-1])],
        vec![saved_effect("minecraft:breath_of_the_nautilus", &[36])],
        vec![
            saved_effect("minecraft:breath_of_the_nautilus", &[100]),
            saved_effect("minecraft:water_breathing", &[40]),
        ],
        vec![saved_effect("minecraft:water_breathing", &[2, 12])],
        vec![Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:water_breathing".into())),
            ("duration".into(), Tag::Int(2)),
            ("amplifier".into(), Tag::Byte(2)),
            ("show_particles".into(), Tag::Byte(1)),
            (
                "hidden_effect".into(),
                Tag::Compound(vec![
                    ("duration".into(), Tag::Int(12)),
                    ("amplifier".into(), Tag::Byte(1)),
                    ("ambient".into(), Tag::Byte(1)),
                    ("show_particles".into(), Tag::Byte(0)),
                ]),
            ),
        ])],
        vec![saved_effect("minecraft:conduit_power", &[0])],
        vec![{
            let Tag::Compound(mut effect) = saved_effect("minecraft:water_breathing", &[20]) else {
                unreachable!()
            };
            effect.push(("amplifier".into(), Tag::Byte(-1)));
            Tag::Compound(effect)
        }],
    ];
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("AirOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    let mut cases = Vec::new();
    let mut players = Vec::new();
    for saved in &effects {
        for base in [0.0, 0.5, 2.0, 100.0] {
            for invulnerable in [false, true] {
                for pattern in 0..2 {
                    for air in [0, 299] {
                        let mut attribute = vec![
                            ("id".into(), Tag::String("minecraft:oxygen_bonus".into())),
                            ("base".into(), Tag::Double(base)),
                        ];
                        if base > 0.0 {
                            let modifier = |id: &str, operation: &str, amount: f64| {
                                Tag::Compound(vec![
                                    ("id".into(), Tag::String(id.into())),
                                    ("operation".into(), Tag::String(operation.into())),
                                    ("amount".into(), Tag::Double(amount)),
                                ])
                            };
                            attribute.push((
                                "modifiers".into(),
                                Tag::List {
                                    element_type: TagType::Compound,
                                    values: vec![
                                        modifier("example:add", "add_value", 0.5),
                                        modifier("example:base", "add_multiplied_base", 0.25),
                                        modifier("example:total", "add_multiplied_total", 0.25),
                                    ],
                                },
                            ));
                        }
                        let document = NamedTag {
                            name: "".into(),
                            value: Tag::Compound(vec![
                                (
                                    "active_effects".into(),
                                    Tag::List {
                                        element_type: TagType::Compound,
                                        values: saved.clone(),
                                    },
                                ),
                                (
                                    "attributes".into(),
                                    Tag::List {
                                        element_type: TagType::Compound,
                                        values: vec![Tag::Compound(attribute)],
                                    },
                                ),
                            ]),
                        };
                        let index = cases.len();
                        let seed = 91_213_i64 + index as i64 * 31;
                        std::fs::write(
                            temp.path().join(format!("{index}.nbt")),
                            native_persistence::nbt::write(&document).unwrap(),
                        )
                        .unwrap();
                        cases.push(json!({"air":air,"health":20.0,"absorption":0.0,"invulnerable":invulnerable,"pattern":pattern,"seed":seed}));
                        players.push(document);
                    }
                }
            }
        }
    }
    std::fs::write(&input, serde_json::to_vec(&cases).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("AirOracle")
        .arg(&input)
        .arg(&output)
        .arg(temp.path()));
    let expected: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let mode = if case["invulnerable"].as_bool().unwrap() {
            GameMode::Creative
        } else {
            GameMode::Survival
        };
        let mut vitals = PlayerVitals {
            air: case["air"].as_i64().unwrap() as i32,
            ..Default::default()
        };
        let mut effects = read_saved_breathing_effects(&players[index]).unwrap();
        let bonus = read_saved_oxygen_bonus(&players[index]).unwrap();
        let mut random = LegacyRandom::new(case["seed"].as_i64().unwrap());
        for tick in 0..400 {
            let pattern = case["pattern"].as_i64().unwrap();
            let eyes = pattern == 0 || tick < 340 || (360..380).contains(&tick);
            let bubble = pattern != 0 && (320..340).contains(&tick);
            vitals.tick_air_with(eyes, bubble, mode, &effects, bonus, &mut || {
                random.next_double()
            });
            effects.tick();
            let expected = &expected[index][tick];
            let draft = NamedTag {
                name: "".into(),
                value: Tag::Compound(vec![(
                    "active_effects".into(),
                    advance_saved_effects(&players[index], tick as u64 + 1)
                        .unwrap()
                        .unwrap(),
                )]),
            };
            assert_eq!(
                read_saved_breathing_effects(&draft).unwrap(),
                effects,
                "draft case {index} tick {tick}"
            );
            assert_eq!(
                vitals.air,
                expected[0].as_i64().unwrap() as i32,
                "case {index} tick {tick}, bonus {bonus}, effects {effects:?}"
            );
            assert_eq!(
                vitals.health,
                expected[1].as_f64().unwrap() as f32,
                "case {index} tick {tick}"
            );
            assert_eq!(
                vitals.absorption,
                expected[2].as_f64().unwrap() as f32,
                "case {index} tick {tick}"
            );
            for (effect, position) in [
                (&effects.water_breathing, 3),
                (&effects.conduit_power, 4),
                (&effects.breath_of_the_nautilus, 5),
            ] {
                assert_eq!(
                    effect.as_ref().map_or(-99_999, |effect| effect.duration),
                    expected[position].as_i64().unwrap() as i32,
                    "case {index} tick {tick}"
                );
            }
            if let Some(Tag::List { values, .. }) = draft.value.get("active_effects") {
                for (offset, name) in [
                    "minecraft:water_breathing",
                    "minecraft:conduit_power",
                    "minecraft:breath_of_the_nautilus",
                ]
                .iter()
                .enumerate()
                {
                    let saved = values.iter().find(|value| matches!(value.get("id"), Some(Tag::String(id)) if id.matches(name)));
                    let details = expected[6 + offset].as_array().unwrap();
                    assert_eq!(saved.is_some(), !details.is_empty());
                    if let Some(saved) = saved {
                        let amplifier = match saved.get("amplifier") {
                            Some(Tag::Byte(v)) => *v as u8,
                            _ => 0,
                        };
                        let flag = |key: &str, default| match saved.get(key) {
                            Some(Tag::Byte(v)) => *v != 0,
                            _ => default,
                        };
                        let visible = flag("show_particles", true);
                        assert_eq!(u64::from(amplifier), details[0].as_u64().unwrap());
                        assert_eq!(flag("ambient", false), details[1].as_bool().unwrap());
                        assert_eq!(visible, details[2].as_bool().unwrap());
                        assert_eq!(flag("show_icon", visible), details[3].as_bool().unwrap());
                    }
                }
            }
        }
    }
    println!(
        "Compared {} ticks with saved breathing effects and oxygen attributes",
        cases.len() * 400
    );
}
