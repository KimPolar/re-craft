//! Compare fluid contact and current against 26.3's EntityFluidInteraction.

use glam::{DVec3, IVec3};
use native_world::{
    fluid::{self, Cell, FluidState},
    loaded::BlockState,
    movement::BoundingBox,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.material.*;
import net.minecraft.world.phys.*;

public class ContactOracle {
    static class TestSection extends LevelChunkSection {
        TestSection() { super((PalettedContainerFactory)null); }
        @Override public boolean hasFluid() { return true; }
    }
    static class TestChunk extends LevelChunk {
        LevelChunkSection[] testSections;
        TestChunk() { super(null, null); }
        @Override public LevelChunkSection[] getSections() { return testSections; }
        @Override public int getSectionIndexFromSectionY(int y) { return 0; }
    }
    static class TestLevel extends ClientLevel {
        Map<BlockPos,BlockState> blocks;
        TestChunk chunk;
        TestLevel() { super(null, null, null, null, 0, 0, null, false, 0L, 0); }
        @Override public ChunkAccess getChunk(int x, int z, ChunkStatus status, boolean create) { return chunk; }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos, Blocks.AIR.defaultBlockState()); }
        @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    }
    static class TestPlayer extends Player {
        Vec3 velocity;
        double eye;
        TestPlayer() { super(null, null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public double getEyeY() { return eye; }
        @Override public AABB getFluidInteractionBox() { return getBoundingBox().deflate(0.001); }
        @Override public Vec3 getDeltaMovement() { return velocity; }
        @Override public void setDeltaMovement(Vec3 v) { velocity = v; }
        void setup(TestLevel level, AABB bounds, double eyeY, Vec3 v) { setLevel(level); setBoundingBox(bounds); eye = eyeY; velocity = v; }
    }
    static AABB box(JsonArray a) { return new AABB(a.get(0).getAsDouble(),a.get(1).getAsDouble(),a.get(2).getAsDouble(),a.get(3).getAsDouble(),a.get(4).getAsDouble(),a.get(5).getAsDouble()); }
    static Vec3 vec(JsonArray a) { return new Vec3(a.get(0).getAsDouble(),a.get(1).getAsDouble(),a.get(2).getAsDouble()); }
    static JsonArray vec(Vec3 v) { var a = new JsonArray(); a.add(v.x); a.add(v.y); a.add(v.z); return a; }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID, Map.of(
            FluidTags.WATER,List.of(Fluids.WATER.builtInRegistryHolder(),Fluids.FLOWING_WATER.builtInRegistryHolder()),
            FluidTags.LAVA,List.of(Fluids.LAVA.builtInRegistryHolder(),Fluids.FLOWING_LAVA.builtInRegistryHolder())))).apply();
        BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, Map.of(BlockTags.BLOCKS_FLUID_FLOW,List.of(Blocks.STONE.builtInRegistryHolder(),Blocks.ICE.builtInRegistryHolder())))).apply();
        Field f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); sun.misc.Unsafe unsafe = (sun.misc.Unsafe)f.get(null);
        TestLevel level = (TestLevel)unsafe.allocateInstance(TestLevel.class);
        TestChunk chunk = (TestChunk)unsafe.allocateInstance(TestChunk.class);
        chunk.testSections = new LevelChunkSection[]{(TestSection)unsafe.allocateInstance(TestSection.class)}; level.chunk = chunk;
        TestPlayer player = (TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        Field blockPos = Entity.class.getDeclaredField("blockPosition"); blockPos.setAccessible(true);
        EntityFluidInteraction interaction = new EntityFluidInteraction(Set.of(FluidTags.WATER, FluidTags.LAVA));
        JsonArray scenarios = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray output = new JsonArray();
        for (var raw : scenarios) {
            var s = raw.getAsJsonObject(); level.blocks = new HashMap<>();
            for (var item : s.getAsJsonArray("blocks")) {
                var a = item.getAsJsonArray(); int code = a.get(3).getAsInt();
                BlockState state = code == 1 ? Blocks.STONE.defaultBlockState() : code == 2 ? Blocks.ICE.defaultBlockState() : code == 40 ? BuiltInRegistries.BLOCK.getValue(net.minecraft.resources.Identifier.parse("minecraft:copper_grate")).defaultBlockState().setValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.WATERLOGGED, true) : (code < 20 ? Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, code-3) : Blocks.LAVA.defaultBlockState().setValue(LiquidBlock.LEVEL, code-20));
                level.blocks.put(new BlockPos(a.get(0).getAsInt(),a.get(1).getAsInt(),a.get(2).getAsInt()),state);
            }
            AABB box = box(s.getAsJsonArray("bounds"));
            player.setup(level,box,s.get("eye").getAsDouble(),vec(s.getAsJsonArray("velocity")));
            blockPos.set(player, BlockPos.containing((box.minX+box.maxX)*0.5,box.minY,(box.minZ+box.maxZ)*0.5));
            interaction.update(player,false);
            double water = interaction.getFluidHeight(FluidTags.WATER), lava = interaction.getFluidHeight(FluidTags.LAVA);
            boolean eyesWater = interaction.isEyeInFluid(FluidTags.WATER), eyesLava = interaction.isEyeInFluid(FluidTags.LAVA);
            interaction.applyCurrentTo(FluidTags.WATER,player,0.014);
            interaction.applyCurrentTo(FluidTags.LAVA,player,0.0023333333333333335);
            JsonObject result = new JsonObject(); result.addProperty("water",water); result.addProperty("lava",lava); result.addProperty("eyesWater",eyesWater); result.addProperty("eyesLava",eyesLava); result.add("velocity",vec(player.velocity)); output.add(result);
        }
        Files.writeString(Path.of(args[1]),new Gson().toJson(output));
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
            let p = game.join("libraries").join(path);
            if p.is_file() {
                paths.push(p);
            }
        }
    }
    std::env::join_paths(paths).unwrap()
}
fn run(command: &mut Command) {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "fluid contact oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
fn vec3(value: &Value) -> DVec3 {
    DVec3::new(
        value[0].as_f64().unwrap(),
        value[1].as_f64().unwrap(),
        value[2].as_f64().unwrap(),
    )
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn fluid_contacts_and_currents_match_vanilla() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut random = 91817_u32;
    let mut next = || {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        random
    };
    let mut scenarios = Vec::new();
    for i in 0..1024 {
        let x = f64::from(next() % 1800) / 1000.0 - 0.9;
        let z = f64::from(next() % 1800) / 1000.0 - 0.9;
        let y = f64::from(next() % 2600) / 1000.0 - 0.3;
        let height = [1.8_f32, 1.5, 0.6][i % 3];
        let eye = [1.62_f32, 1.27, 0.4][i % 3];
        let half = f64::from(0.3_f32);
        let mut blocks = Vec::new();
        for bx in -2..=2 {
            for by in -1..=3 {
                for bz in -2..=2 {
                    let n = next() % 12;
                    let code = match n {
                        0 => 1,
                        1 => 2,
                        2..=6 => 3 + (next() % 16),
                        7 => 20 + (next() % 16),
                        _ => 0,
                    };
                    if code != 0 {
                        blocks.push([bx, by, bz, code as i32]);
                    }
                }
            }
        }
        let velocity = if i % 3 == 0 {
            [0.0, 0.0, 0.0]
        } else {
            [
                f64::from(next() % 40) / 1000.0 - 0.02,
                f64::from(next() % 40) / 1000.0 - 0.02,
                f64::from(next() % 40) / 1000.0 - 0.02,
            ]
        };
        scenarios.push(json!({"bounds":[x-half,y,z-half,x+half,y+f64::from(height),z+half],"eye":y+f64::from(eye),"velocity":velocity,"blocks":blocks}));
    }
    // A waterlogged grate is both a source and falling; a sturdy ceiling
    // therefore gives it full camera height even though the falling flag is set.
    for (ceiling, eye) in [(true, 0.95), (true, 0.85), (false, 0.95)] {
        let mut blocks = vec![[0, 0, 0, 40]];
        if ceiling {
            blocks.push([0, 1, 0, 1]);
        }
        scenarios.push(json!({"bounds":[0.2,0.1,0.2,0.8,1.2,0.8],"eye":eye,"velocity":[0.0,0.0,0.0],"blocks":blocks}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ContactOracle.java");
    let input = temp.path().join("input.json");
    let output = temp.path().join("output.json");
    std::fs::write(&source, ORACLE).unwrap();
    std::fs::write(&input, serde_json::to_vec(&scenarios).unwrap()).unwrap();
    let cp = classpath(&game, temp.path());
    let java = java_bin();
    run(Command::new(java.join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    run(Command::new(java.join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("ContactOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    assert_eq!(scenarios.len(), expected.len());
    let mut errors = Vec::new();
    let mut immersed = 0;
    for (i, (input, expected)) in scenarios.iter().zip(&expected).enumerate() {
        let mut blocks = HashMap::new();
        for a in input["blocks"].as_array().unwrap() {
            let p = IVec3::new(
                a[0].as_i64().unwrap() as i32,
                a[1].as_i64().unwrap() as i32,
                a[2].as_i64().unwrap() as i32,
            );
            let code = a[3].as_i64().unwrap();
            blocks.insert(p, code);
        }
        let cell = |p| match blocks.get(&p).copied().unwrap_or(0) {
            0 => Cell::Empty,
            1 | 2 => Cell::Solid,
            40 => Cell::Fluid(
                FluidState::from_block(&BlockState {
                    name: "minecraft:copper_grate".into(),
                    properties: BTreeMap::from([("waterlogged".into(), "true".into())]),
                })
                .unwrap()
                .unwrap(),
            ),
            code => {
                let (name, level) = if code < 20 {
                    ("minecraft:water", code - 3)
                } else {
                    ("minecraft:lava", code - 20)
                };
                let state = BlockState {
                    name: name.into(),
                    properties: BTreeMap::from([("level".into(), level.to_string())]),
                };
                Cell::Fluid(FluidState::from_block(&state).unwrap().unwrap())
            }
        };
        let sturdy = |p, dir| match blocks.get(&p).copied().unwrap_or(0) {
            1 => true,
            2 => dir == IVec3::NEG_Y,
            _ => false,
        };
        let b = &input["bounds"];
        let bounds = BoundingBox {
            min: vec3(b),
            max: DVec3::new(
                b[3].as_f64().unwrap(),
                b[4].as_f64().unwrap(),
                b[5].as_f64().unwrap(),
            ),
        };
        let contacts = fluid::contacts(bounds, input["eye"].as_f64().unwrap(), &cell, &sturdy);
        let v = contacts.water.push(vec3(&input["velocity"]), 0.014);
        let v = contacts.lava.push(v, 0.0023333333333333335);
        if contacts.water.depth > 0.0 || contacts.lava.depth > 0.0 {
            immersed += 1;
        }
        if ((contacts.water.depth - expected["water"].as_f64().unwrap()).abs() > 1.0e-10
            || (contacts.lava.depth - expected["lava"].as_f64().unwrap()).abs() > 1.0e-10
            || contacts.water.eyes_inside != expected["eyesWater"].as_bool().unwrap()
            || contacts.lava.eyes_inside != expected["eyesLava"].as_bool().unwrap()
            || (v - vec3(&expected["velocity"])).abs().max_element() > 1.0e-9)
            && errors.len() < 15
        {
            errors.push(format!(
                "case {i}: native {contacts:?} velocity {v:?}, Java {expected}, input {input}"
            ));
        }
    }
    assert!(immersed > 100);
    assert!(
        errors.is_empty(),
        "fluid contact mismatches:\n{}",
        errors.join("\n")
    );
    println!(
        "Compared {} fluid contact and current cases with Minecraft 26.3",
        scenarios.len()
    );
}
