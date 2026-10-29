//! Opt-in comparison against fluid state and surface calculations in the local 26.3 JAR.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use glam::{DVec3, IVec3};
use native_world::{
    fluid::{self, Cell, FluidKind, FluidState},
    loaded::BlockState,
};
use serde_json::Value;

const ORACLE: &str = r#"
import java.io.PrintStream;
import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.client.renderer.block.BlockAndTintGetter;
import net.minecraft.client.renderer.block.FluidRenderer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.LiquidBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.*;

public class FluidOracle {
  public static void main(String[] args) throws Exception {
    PrintStream output = System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    for (int kind = 0; kind < 2; kind++) for (int level = 0; level < 16; level++) {
      var block = (kind == 0 ? Blocks.WATER : Blocks.LAVA).defaultBlockState().setValue(LiquidBlock.LEVEL, level);
      var fluid = block.getFluidState();
      output.printf(Locale.ROOT, "L %d %d %d %d %.9f%n", kind, level, fluid.getAmount(), fluid.getValue(FlowingFluid.FALLING) ? 1 : 0, fluid.getOwnHeight());
    }
    var slab = Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.WATERLOGGED, true).getFluidState();
    output.printf(Locale.ROOT, "W %d %.9f%n", slab.getAmount(), slab.getOwnHeight());
    Map<BlockPos,BlockState> blocks = new HashMap<>();
    BlockAndTintGetter world = (BlockAndTintGetter)Proxy.newProxyInstance(FluidOracle.class.getClassLoader(),
      new Class[]{BlockAndTintGetter.class}, (proxy, method, argv) -> {
        if (method.getName().equals("getBlockState")) return blocks.getOrDefault((BlockPos)argv[0], Blocks.AIR.defaultBlockState());
        if (method.getName().equals("getFluidState")) return blocks.getOrDefault((BlockPos)argv[0], Blocks.AIR.defaultBlockState()).getFluidState();
        throw new UnsupportedOperationException(method.getName());
      });
    var renderer = new FluidRenderer(null);
    var sample = FluidRenderer.class.getDeclaredMethod("getHeight", BlockAndTintGetter.class, Fluid.class, BlockPos.class);
    sample.setAccessible(true);
    var corner = FluidRenderer.class.getDeclaredMethod("calculateAverageHeight", BlockAndTintGetter.class, Fluid.class, float.class, float.class, float.class, BlockPos.class);
    corner.setAccessible(true);
    var diagonal = new BlockPos(1,0,1);
    float[] edges = {-1f, 0f, 1f/9f, 5f/9f, 8f/9f, 1f};
    for (int mode = 0; mode < 6; mode++) {
      blocks.clear();
      if (mode == 1) blocks.put(diagonal, Blocks.STONE.defaultBlockState());
      if (mode == 2 || mode == 4) blocks.put(diagonal, Blocks.WATER.defaultBlockState());
      if (mode == 3) blocks.put(diagonal, Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 3));
      if (mode == 4) blocks.put(diagonal.above(), Blocks.WATER.defaultBlockState());
      if (mode == 5) blocks.put(diagonal, Blocks.LAVA.defaultBlockState());
      float value = (Float)sample.invoke(renderer, world, Fluids.WATER, diagonal);
      output.printf(Locale.ROOT, "H %d %.9f%n", mode, value);
      for (int a = 0; a < edges.length; a++) for (int b = 0; b < edges.length; b++) {
        float result = (Float)corner.invoke(renderer, world, Fluids.WATER, 8f/9f, edges[a], edges[b], diagonal);
        output.printf(Locale.ROOT, "C %d %d %d %.9f%n", mode, a, b, result);
      }
    }
    int[][] sides = {{0,-1},{1,0},{0,1},{-1,0}};
    var origin = BlockPos.ZERO;
    for (int i = 0; i < 512; i++) {
      blocks.clear();
      var center = Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, i % 8);
      blocks.put(origin, center);
      for (int j = 0; j < sides.length; j++) {
        var pos = origin.offset(sides[j][0], 0, sides[j][1]);
        int code = (i >> (j * 2)) & 3;
        if (code == 1) blocks.put(pos, Blocks.WATER.defaultBlockState());
        if (code == 2) blocks.put(pos, Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 3));
        if (code == 3) blocks.put(pos, Blocks.LAVA.defaultBlockState());
        if (code == 0 && (i & 256) != 0) blocks.put(pos.below(), Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL, 3));
      }
      var flow = center.getFluidState().getFlow(world, origin);
      output.printf(Locale.ROOT, "F %d %.9f %.9f%n", i, flow.x, flow.z);
    }
  }
}
"#;

const REGISTRY_ORACLE: &str = r#"
import java.io.PrintStream;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.Fluids;

public class FluidRegistryOracle {
  public static void main(String[] args) {
    PrintStream output = System.out;
    SharedConstants.tryDetectVersion();
    Bootstrap.bootStrap();
    for (var block : BuiltInRegistries.BLOCK) {
      for (var state : block.getStateDefinition().getPossibleStates()) {
        var fluid = state.getFluidState();
        var type = fluid.getType();
        String kind;
        if (fluid.isEmpty()) kind = "empty";
        else if (type == Fluids.WATER || type == Fluids.FLOWING_WATER) kind = "water";
        else if (type == Fluids.LAVA || type == Fluids.FLOWING_LAVA) kind = "lava";
        else throw new IllegalStateException("unknown fluid " + type);
        var props = state.getValues()
          .map(value -> value.property().getName() + "=" + value.valueName()).sorted().toList();
        output.printf("S\t%s\t%s\t%s\t%s\t%d\t%s\t%s%n",
          BuiltInRegistries.BLOCK.getKey(block), String.join(",", props), state.isAir(), kind,
          fluid.isEmpty() ? 0 : fluid.getAmount(), fluid.isSource(),
          fluid.isEmpty() ? false : fluid.getValue(FlowingFluid.FALLING));
      }
    }
  }
}
"#;

fn game_dir() -> PathBuf {
    std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"))
}

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
    assert_eq!(manifest["id"], "26.3");
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

fn run(command: &mut Command) -> String {
    let output = command.output().expect("cannot run Java 25");
    assert!(
        output.status.success(),
        "Java fluid oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn item_current_matches_java_accumulator_in_shallow_flow() {
    use native_world::movement::BoundingBox;
    let java_source = r#"
import java.lang.reflect.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.phys.Vec3;
public class ItemCurrentOracle {
  static class Item extends ItemEntity {
    Vec3 velocity;
    Item() { super(null,0,0,0,ItemStack.EMPTY); }
    @Override public Vec3 getDeltaMovement() { return velocity; }
    @Override public void setDeltaMovement(Vec3 v) { velocity=v; }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var cls=Class.forName("net.minecraft.world.entity.EntityFluidInteraction$CurrentAccumulator");
    var ctor=cls.getDeclaredConstructor(); ctor.setAccessible(true);
    var add=cls.getDeclaredMethod("accumulate",Vec3.class); add.setAccessible(true);
    var apply=cls.getDeclaredMethod("applyTo",net.minecraft.world.entity.Entity.class,double.class); apply.setAccessible(true);
    for (double speed : new double[]{0.0,0.1}) for (double strength : new double[]{0.014,0.007,0.0023333333333333335}) {
      var acc=ctor.newInstance(); add.invoke(acc,new Vec3(Double.parseDouble(args[0]),0,0));
      var item=(Item)unsafe.allocateInstance(Item.class); item.velocity=new Vec3(speed,0,0);
      apply.invoke(acc,item,strength);
      System.out.printf(java.util.Locale.ROOT,"RESULT %.17g %.17g %.17g%n",item.velocity.x,item.velocity.y,item.velocity.z);
    }
  }
}

"#;
    let game = game_dir();
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("ItemCurrentOracle.java");
    std::fs::write(&source, java_source).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let source_fluid = FluidState {
        kind: FluidKind::Water,
        amount: 8,
        source: true,
        falling: false,
    };
    let edge = FluidState {
        kind: FluidKind::Water,
        amount: 4,
        source: false,
        falling: false,
    };
    let world = |pos| {
        if pos == IVec3::ZERO {
            Cell::Fluid(source_fluid)
        } else if pos == IVec3::X {
            Cell::Fluid(edge)
        } else {
            Cell::Solid
        }
    };
    let surface = f64::from(source_fluid.own_height());
    let bounds = BoundingBox {
        min: DVec3::new(0.2, surface - 0.2, 0.2),
        max: DVec3::new(0.8, surface + 0.05, 0.8),
    };
    let contacts = fluid::contacts(bounds, bounds.min.y, &world, &|_, _| false);
    let flow = fluid::flow(&world, &|_, _| false, IVec3::ZERO, source_fluid);
    assert!((flow.x - 1.0).abs() < 1e-12);
    let scalar = contacts.water.depth;
    let output = run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(cp)
        .arg("ItemCurrentOracle")
        .arg(scalar.to_string()));
    let rows: Vec<_> = output
        .lines()
        .filter_map(|line| line.strip_prefix("RESULT "))
        .map(|line| {
            line.split_whitespace()
                .map(|value| value.parse::<f64>().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(rows.len(), 6);
    for (index, (speed, strength)) in [0.0, 0.1]
        .into_iter()
        .flat_map(|speed| {
            [0.014, 0.007, 0.0023333333333333335]
                .into_iter()
                .map(move |strength| (speed, strength))
        })
        .enumerate()
    {
        let actual = contacts
            .water
            .push_entity(DVec3::new(speed, 0.0, 0.0), strength);
        let expected = DVec3::new(rows[index][0], rows[index][1], rows[index][2]);
        assert!(
            (actual - expected).abs().max_element() < 1e-12,
            "{speed} {strength}: {actual:?} vs {expected:?}"
        );
    }
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn all_vanilla_block_states_have_matching_fluid_and_air_flags() {
    let game = game_dir();
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let java = java_bin();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FluidRegistryOracle.java");
    std::fs::write(&source, REGISTRY_ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("FluidRegistryOracle"));
    let mut checked = 0usize;
    let mut mismatches = BTreeMap::<String, (BTreeMap<String, String>, usize)>::new();
    for line in output.lines().filter(|line| line.starts_with("S\t")) {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields.len(), 8, "{line}");
        let properties = fields[2]
            .split(',')
            .filter(|part| !part.is_empty())
            .map(|part| {
                let (name, value) = part.split_once('=').unwrap();
                (name.to_owned(), value.to_owned())
            })
            .collect();
        let state = BlockState {
            name: fields[1].to_owned(),
            properties,
        };
        let is_air = matches!(
            state.name.as_str(),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        );
        assert_eq!(is_air, fields[3] == "true", "air flag for {state:?}");
        let actual = FluidState::from_block(&state).unwrap();
        let expected = match fields[4] {
            "empty" => None,
            "water" | "lava" => Some(FluidState {
                kind: if fields[4] == "water" {
                    FluidKind::Water
                } else {
                    FluidKind::Lava
                },
                amount: fields[5].parse().unwrap(),
                source: fields[6] == "true",
                falling: fields[7] == "true",
            }),
            _ => panic!("unexpected Java fluid: {line}"),
        };
        if actual != expected {
            let key = format!("{}: Rust {actual:?}, Java {expected:?}", state.name);
            let item = mismatches.entry(key).or_insert((state.properties, 0));
            item.1 += 1;
        }
        checked += 1;
    }
    assert!(checked > 30_000, "only {checked} block states compared");
    assert!(mismatches.is_empty(), "fluid mismatches: {mismatches:#?}");
    println!("Compared {checked} vanilla block states");
}

#[test]
#[ignore = "requires the user's Minecraft Java Edition 26.3 installation and Java 25 JDK"]
fn fluid_states_and_corner_heights_match_vanilla() {
    let game = game_dir();
    let java = java_bin();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FluidOracle.java");
    std::fs::write(&source, ORACLE).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java.join("javac.exe"))
        .current_dir(temp.path())
        .arg("-encoding")
        .arg("UTF-8")
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = run(Command::new(java.join("java.exe"))
        .current_dir(temp.path())
        .arg("-classpath")
        .arg(&cp)
        .arg("FluidOracle"));

    let water = |level: u8| {
        Cell::Fluid(
            FluidState::from_block(&BlockState {
                name: "minecraft:water".into(),
                properties: BTreeMap::from([("level".into(), level.to_string())]),
            })
            .unwrap()
            .unwrap(),
        )
    };
    let modes = [
        Cell::Empty,
        Cell::Solid,
        water(0),
        water(3),
        water(0),
        Cell::Fluid(FluidState {
            kind: FluidKind::Lava,
            amount: 8,
            source: true,
            falling: false,
        }),
    ];
    let edges = [-1.0f32, 0.0, 1.0 / 9.0, 5.0 / 9.0, 8.0 / 9.0, 1.0];
    let diagonal = IVec3::new(1, 0, 1);
    let mut checked = 0;
    for line in output.lines() {
        let parts: Vec<_> = line.split_whitespace().collect();
        match parts.first().copied() {
            Some("L") => {
                let kind: usize = parts[1].parse().unwrap();
                let level: u8 = parts[2].parse().unwrap();
                let state = FluidState::from_block(&BlockState {
                    name: if kind == 0 {
                        "minecraft:water"
                    } else {
                        "minecraft:lava"
                    }
                    .into(),
                    properties: BTreeMap::from([("level".into(), level.to_string())]),
                })
                .unwrap()
                .unwrap();
                assert_eq!(state.amount, parts[3].parse::<u8>().unwrap());
                assert_eq!(state.falling, parts[4] == "1");
                assert!((state.own_height() - parts[5].parse::<f32>().unwrap()).abs() < 1e-6);
            }
            Some("W") => {
                let state = FluidState::from_block(&BlockState {
                    name: "minecraft:oak_slab".into(),
                    properties: BTreeMap::from([("waterlogged".into(), "true".into())]),
                })
                .unwrap()
                .unwrap();
                assert_eq!(state.amount, parts[1].parse::<u8>().unwrap());
                assert!((state.own_height() - parts[2].parse::<f32>().unwrap()).abs() < 1e-6);
            }
            Some("H") | Some("C") => {
                let mode: usize = parts[1].parse().unwrap();
                let sample_world = |position: IVec3| {
                    if position == diagonal {
                        modes[mode]
                    } else if mode == 4 && position == diagonal + IVec3::Y {
                        water(0)
                    } else {
                        Cell::Empty
                    }
                };
                let h = fluid::height(&sample_world, diagonal, FluidKind::Water);
                let (actual, expected) = if parts[0] == "H" {
                    (h, parts[2].parse::<f32>().unwrap())
                } else {
                    let a: usize = parts[2].parse().unwrap();
                    let b: usize = parts[3].parse().unwrap();
                    (
                        fluid::corner_height(8.0 / 9.0, edges[a], edges[b], Some(h)),
                        parts[4].parse::<f32>().unwrap(),
                    )
                };
                assert!(
                    (actual - expected).abs() < 1e-6,
                    "{line}: Rust {actual}, Java {expected}"
                );
            }
            Some("F") => {
                let case: usize = parts[1].parse().unwrap();
                let center = match water((case % 8) as u8) {
                    Cell::Fluid(state) => state,
                    _ => unreachable!(),
                };
                let sides = [IVec3::NEG_Z, IVec3::X, IVec3::Z, IVec3::NEG_X];
                let sample_world = |position: IVec3| {
                    if position == IVec3::ZERO {
                        return Cell::Fluid(center);
                    }
                    for (index, direction) in sides.iter().enumerate() {
                        let code = (case >> (index * 2)) & 3;
                        if position == *direction {
                            return match code {
                                1 => water(0),
                                2 => water(3),
                                3 => modes[5],
                                _ => Cell::Empty,
                            };
                        }
                        if position == *direction - IVec3::Y && code == 0 && (case & 256) != 0 {
                            return water(3);
                        }
                    }
                    Cell::Empty
                };
                let actual = fluid::horizontal_flow(&sample_world, IVec3::ZERO, center);
                for (result, expected) in actual.into_iter().zip(&parts[2..]) {
                    let expected: f64 = expected.parse().unwrap();
                    assert!(
                        (result - expected).abs() < 1e-6,
                        "{line}: Rust {result}, Java {expected}"
                    );
                }
            }
            _ => continue,
        }
        checked += 1;
    }
    assert_eq!(checked, 32 + 1 + 6 + 6 * 6 * 6 + 512);
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn fast_crossing_fluid_order_matches_java_block_traversal() {
    use native_world::movement::BoundingBox;
    const JAVA: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
public class FluidSweepOracle {
  public static void main(String[] args) throws Exception {
    JsonArray out=new JsonArray();
    for(boolean reverse:new boolean[]{false,true}) {
      Vec3 from=new Vec3(reverse?4:0,0.1,0.5), to=new Vec3(reverse?0:4,0.1,0.5);
      AABB start=new AABB(from.x-.125,from.y,from.z-.125,from.x+.125,from.y+.25,from.z+.125);
      AABB end=start.move(to.subtract(from)).deflate((double)1.0e-5f);
      JsonArray hits=new JsonArray(); Set<Long> seen=new HashSet<>();
      BlockGetter.forEachBlockIntersectedBetween(from,to,end,(pos,step)-> {
        if(pos.getY()==0 && pos.getZ()==0 && (pos.getX()==1 || pos.getX()==2) && seen.add(pos.asLong())) {
          AABB fluid=new AABB(pos.getX(),0,pos.getZ(),pos.getX()+1,8.0f/9.0f,pos.getZ()+1);
          if(start.collidedAlongVector(to.subtract(from),List.of(fluid))) hits.add(pos.getX()==1 ? "lava" : "water");
        }
        return true;
      }); out.add(hits);
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(out));
  }
}

"#;
    let game = game_dir();
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FluidSweepOracle.java");
    std::fs::write(&source, JAVA).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let output = temp.path().join("sweep.json");
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg("FluidSweepOracle")
        .arg(&output));
    let expected: Vec<Vec<String>> =
        serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let state = |kind| FluidState {
        kind,
        amount: 8,
        source: true,
        falling: false,
    };
    let world = |pos: IVec3| match (pos.x, pos.y, pos.z) {
        (1, 0, 0) => Cell::Fluid(state(FluidKind::Lava)),
        (2, 0, 0) => Cell::Fluid(state(FluidKind::Water)),
        _ => Cell::Empty,
    };
    for (index, reverse) in [false, true].into_iter().enumerate() {
        let x = if reverse { 4.0 } else { 0.0 };
        let bounds = BoundingBox {
            min: DVec3::new(x - 0.125, 0.1, 0.375),
            max: DVec3::new(x + 0.125, 0.35, 0.625),
        };
        let movement = DVec3::new(if reverse { -4.0 } else { 4.0 }, 0.0, 0.0);
        let actual: Vec<_> = fluid::swept_effects(bounds, movement, movement, &world)
            .unwrap()
            .into_iter()
            .map(|kind| match kind {
                FluidKind::Water => "water",
                FluidKind::Lava => "lava",
            })
            .collect();
        assert_eq!(actual, expected[index]);
    }
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn diagonal_fluid_order_matches_java_axis_traversal() {
    use native_world::movement::BoundingBox;
    const JAVA: &str = r#"
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;
public class DiagonalFluidOracle {
  static Vec3 v(JsonArray a) { return new Vec3(a.get(0).getAsDouble(),a.get(1).getAsDouble(),a.get(2).getAsDouble()); }
  public static void main(String[] args) throws Exception {
    var input=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    var output=new JsonArray();
    for(var value:input) {
      var row=value.getAsJsonObject(); Vec3 from=v(row.getAsJsonArray("from")), move=v(row.getAsJsonArray("move")), req=v(row.getAsJsonArray("requested"));
      Map<String,String> cells=new HashMap<>(); for(var cv:row.getAsJsonArray("cells")) { var c=cv.getAsJsonArray(); cells.put(c.get(0)+","+c.get(1)+","+c.get(2),c.get(3).getAsString()); }
      int[] axes=Math.abs(req.x)<Math.abs(req.z)?new int[]{1,2,0}:new int[]{1,0,2};
      Set<Long> seen=new HashSet<>(); JsonArray hits=new JsonArray();
      for(int axis:axes) {
        double d=axis==0?move.x:axis==1?move.y:move.z; if(d==0) continue;
        Map<Integer,List<String>> steps=new TreeMap<>();
        Vec3 to=from.add(axis==0?d:0,axis==1?d:0,axis==2?d:0), delta=to.subtract(from);
        AABB start=new AABB(from.x-.125,from.y,from.z-.125,from.x+.125,from.y+.25,from.z+.125);
        AABB end=start.move(delta).deflate((double)1.0e-5f);
        BlockGetter.forEachBlockIntersectedBetween(from,to,end,(p,step)-> {
          String kind=cells.get(p.getX()+","+p.getY()+","+p.getZ());
          if(kind!=null && !seen.contains(p.asLong())) {
            AABB target=new AABB(p.getX(),p.getY(),p.getZ(),p.getX()+1,p.getY()+8.0f/9.0f,p.getZ()+1);
            if(start.collidedAlongVector(delta,List.of(target))) { seen.add(p.asLong()); steps.computeIfAbsent(step,k->new ArrayList<>()).add(kind); }
          }
          return true;
        });
        for(var kinds:steps.values()) {
          for(var kind:kinds) if(kind.equals("lava")) hits.add(kind);
          if(kinds.contains("water")) hits.add("water");
        }
        from=to;
      }
      output.add(hits);
    }
    Files.writeString(Path.of(args[1]),new Gson().toJson(output));
  }
}
"#;
    let game = game_dir();
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("DiagonalFluidOracle.java");
    std::fs::write(&source, JAVA).unwrap();
    let cp = classpath(&game, temp.path());
    run(Command::new(java_bin().join("javac.exe"))
        .arg("-classpath")
        .arg(&cp)
        .arg(&source));
    let mut cases = Vec::new();
    for sx in [-1, 1] {
        for sz in [-1, 1] {
            for z_first in [false, true] {
                let x = f64::from(sx) * if z_first { 3.0 } else { 4.0 };
                let z = f64::from(sz) * if z_first { 4.0 } else { 3.0 };
                let first = if z_first {
                    IVec3::new(0, 0, sz * 2)
                } else {
                    IVec3::new(sx * 2, 0, 0)
                };
                let second = if z_first {
                    IVec3::new(sx * 2, 0, sz * 4)
                } else {
                    IVec3::new(sx * 4, 0, sz * 2)
                };
                cases.push((
                    DVec3::new(0.5, 0.1, 0.5),
                    DVec3::new(x, 0.0, z),
                    first,
                    second,
                ));
            }
        }
    }
    for lava_z in [0, 1] {
        cases.push((
            DVec3::new(0.5, 0.1, 1.0),
            DVec3::new(4.0, 0.0, 0.0),
            IVec3::new(1, 0, lava_z),
            IVec3::new(1, 0, 1 - lava_z),
        ));
    }
    for z in [0.05, 0.5, 1.0, 1.05] {
        for lava_x in 0..=4 {
            for water_x in 0..=4 {
                for lava_z in 0..=1 {
                    for water_z in 0..=1 {
                        let lava = IVec3::new(lava_x, 0, lava_z);
                        let water = IVec3::new(water_x, 0, water_z);
                        if lava != water {
                            cases.push((
                                DVec3::new(0.5, 0.1, z),
                                DVec3::new(4.0, 0.0, 0.0),
                                lava,
                                water,
                            ));
                        }
                    }
                }
            }
        }
    }
    let input: Vec<_> = cases
        .iter()
        .map(|(from, movement, first, second)| {
            serde_json::json!({
        "from":from.to_array(),"move":movement.to_array(),"requested":movement.to_array(),
        "cells":[[first.x,first.y,first.z,"lava"],[second.x,second.y,second.z,"water"]]})
        })
        .collect();
    let input_path = temp.path().join("diagonal-input.json");
    let output_path = temp.path().join("diagonal-output.json");
    std::fs::write(&input_path, serde_json::to_vec(&input).unwrap()).unwrap();
    run(Command::new(java_bin().join("java.exe"))
        .arg("-classpath")
        .arg(cp)
        .arg("DiagonalFluidOracle")
        .arg(&input_path)
        .arg(&output_path));
    let expected: Vec<Vec<String>> =
        serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert!(expected[..10].iter().all(|hits| hits == &["lava", "water"]));
    for (index, (from, movement, first, second)) in cases.into_iter().enumerate() {
        let state = |kind| FluidState {
            kind,
            amount: 8,
            source: true,
            falling: false,
        };
        let world = |pos| {
            if pos == first {
                Cell::Fluid(state(FluidKind::Lava))
            } else if pos == second {
                Cell::Fluid(state(FluidKind::Water))
            } else {
                Cell::Empty
            }
        };
        let bounds = BoundingBox {
            min: from + DVec3::new(-0.125, 0.0, -0.125),
            max: from + DVec3::new(0.125, 0.25, 0.125),
        };
        let actual: Vec<_> = fluid::swept_effects(bounds, movement, movement, &world)
            .unwrap()
            .into_iter()
            .map(|v| {
                if v == FluidKind::Lava {
                    "lava"
                } else {
                    "water"
                }
            })
            .collect();
        assert_eq!(actual, expected[index], "case {index}");
    }
}
