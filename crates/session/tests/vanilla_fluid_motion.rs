//! Compare default fluid movement against the locally installed 26.3 player.

use glam::DVec3;
use native_session::physics::{FluidMovement, MotionState, MovementInput};
use native_world::fluid::FluidKind;
use serde_json::{Value, json};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

const ORACLE: &str = r#"
import java.nio.file.*;
import java.lang.reflect.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.client.player.LocalPlayer;
import net.minecraft.core.*;
import net.minecraft.world.effect.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.material.*;
import net.minecraft.tags.TagKey;
import net.minecraft.world.phys.*;

public class FluidMotionOracle {
    static Field positionField;
    static class TestLevel extends ClientLevel {
        boolean above;
        TestLevel(){super(null,null,null,null,0,0,null,false,0L,0);}
        @Override public FluidState getFluidState(BlockPos p){ return above ? Fluids.WATER.defaultFluidState() : Fluids.EMPTY.defaultFluidState(); }
    }
    static class TestPlayer extends Player {
        Vec3 p,v; boolean ground,sprint,water,swim,free; float yaw; double look,depth,wall; int delay; AttributeInstance speed;
        TestPlayer(){super(null,null);}
        void setup(TestLevel level,Vec3 pos,Vec3 velocity,boolean grounded,double wallX) throws Exception {setLevel(level);p=pos;v=velocity;ground=grounded;wall=wallX;delay=0;positionField.set(this,p);}
        @Override public GameType gameMode(){return GameType.SURVIVAL;}
        @Override public boolean onGround(){return ground;}
        @Override public boolean isInWater(){return water;}
        @Override public boolean isInLava(){return !water;}
        @Override public boolean isSwimming(){return swim;}
        @Override public boolean isPassenger(){return false;}
        @Override public boolean isVehicle(){return false;}
        @Override public boolean isSprinting(){return sprint;}
        @Override public boolean isAffectedByFluids(){return true;}
        @Override protected boolean shouldTravelInFluid(FluidState state){return true;}
        @Override public boolean onClimbable(){return false;}
        @Override public boolean hasEffect(Holder<MobEffect> effect){return false;}
        @Override public double getFluidHeight(TagKey<Fluid> tag){return depth;}
        @Override public boolean isFree(double x,double y,double z){return free;}
        @Override public Vec3 getLookAngle(){return new Vec3(0,look,0);}
        @Override public Vec3 getDeltaMovement(){return v;}
        @Override public void setDeltaMovement(Vec3 value){v=value;}
        @Override public void setDeltaMovement(double x,double y,double z){v=new Vec3(x,y,z);}
        @Override public float getYRot(){return yaw;}
        @Override protected float getJumpPower(){return 0.42f;}
        @Override protected double getEffectiveGravity(){return 0.08;}
        @Override public double getAttributeValue(Holder<Attribute> a){return a==Attributes.MOVEMENT_SPEED?speed.getValue():a==Attributes.WATER_MOVEMENT_EFFICIENCY?0.0:1.0;}
        @Override public MobEffectInstance getEffect(Holder<MobEffect> e){return null;}
        @Override public HumanoidArm getMainArm(){return HumanoidArm.RIGHT;}
        void down(){goDownInWater();}
        void up(){jumpInLiquid(null);}
        @Override public void move(MoverType type,Vec3 requested){
            double x=requested.x,y=requested.y,z=requested.z;
            if(y<0.0 && p.y+y<0.0)y=-p.y;
            if(x>0.0 && p.x+0.30000001192092896+x>wall)x=Math.max(0.0,wall-p.x-0.30000001192092896);
            p=p.add(x,y,z);try{positionField.set(this,p);}catch(Exception e){throw new RuntimeException(e);}
            boolean bx=Math.abs(x-requested.x)>=(double)1.0e-5f, bz=Math.abs(z-requested.z)>=(double)1.0e-5f, by=y!=requested.y;
            horizontalCollision=bx||bz; ground=by&&requested.y<0.0;
            v=new Vec3(bx?0.0:v.x,by?0.0:v.y,bz?0.0:v.z);
        }
    }
    static Vec3 vec(JsonArray a){return new Vec3(a.get(0).getAsDouble(),a.get(1).getAsDouble(),a.get(2).getAsDouble());}
    static JsonArray vec(Vec3 v){var a=new JsonArray();a.add(v.x);a.add(v.y);a.add(v.z);return a;}
    public static void main(String[] args)throws Exception{
        SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
        Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");f.setAccessible(true);sun.misc.Unsafe unsafe=(sun.misc.Unsafe)f.get(null);
        TestLevel level=(TestLevel)unsafe.allocateInstance(TestLevel.class);TestPlayer entity=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        Field abilities=Player.class.getDeclaredField("abilities");abilities.setAccessible(true);abilities.set(entity,new Abilities());
        positionField=Entity.class.getDeclaredField("position");positionField.setAccessible(true);
        Field eyeHeight=Entity.class.getDeclaredField("eyeHeight");eyeHeight.setAccessible(true);eyeHeight.setFloat(entity,1.62f);
        Field blockPos=Entity.class.getDeclaredField("blockPosition");blockPos.setAccessible(true);blockPos.set(entity,BlockPos.ZERO);
        entity.speed=new AttributeMap(Player.createAttributes().build()).getInstance(Attributes.MOVEMENT_SPEED);
        Field mf=LivingEntity.class.getDeclaredField("SPEED_MODIFIER_SPRINTING");mf.setAccessible(true);AttributeModifier modifier=(AttributeModifier)mf.get(null);
        Method square=LocalPlayer.class.getDeclaredMethod("modifyInputSpeedForSquareMovement",Vec2.class);square.setAccessible(true);
        Field jumping=LivingEntity.class.getDeclaredField("jumping");jumping.setAccessible(true);
        JsonArray scenarios=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(), results=new JsonArray();
        for(var entry:scenarios){var s=entry.getAsJsonObject();entity.setup(level,vec(s.getAsJsonArray("position")),vec(s.getAsJsonArray("velocity")),s.get("ground").getAsBoolean(),s.get("wall").getAsDouble());JsonArray ticks=new JsonArray();
          for(var raw:s.getAsJsonArray("inputs")){var i=raw.getAsJsonObject();entity.water=i.get("water").getAsBoolean();entity.swim=i.get("swim").getAsBoolean();entity.sprint=i.get("sprint").getAsBoolean();entity.look=i.get("look").getAsDouble();entity.depth=i.get("depth").getAsDouble();entity.free=i.get("free").getAsBoolean();entity.yaw=i.get("yaw").getAsFloat();level.above=i.get("above").getAsBoolean();
            entity.speed.removeModifier(modifier.id());if(entity.sprint)entity.speed.addTransientModifier(modifier);
            if(entity.water&&i.get("descend").getAsBoolean())entity.down();
            if(entity.delay>0)entity.delay--; double x=entity.v.x,y=entity.v.y,z=entity.v.z;if(x*x+z*z<9.0e-6){x=0;z=0;}if(Math.abs(y)<0.003)y=0;entity.v=new Vec3(x,y,z);
            boolean jump=i.get("jump").getAsBoolean();jumping.setBoolean(entity,jump);
            if(jump){if(entity.depth>0.0&&(!entity.ground||entity.depth>0.4))entity.up();else if(entity.ground&&entity.delay==0){entity.jumpFromGround();entity.delay=10;}}else entity.delay=0;
            Vec2 key=new Vec2(-i.get("right").getAsFloat(),i.get("forward").getAsFloat()).normalized().scale(0.98f);if(i.get("slow").getAsBoolean())key=key.scale(0.3f);
            Vec2 modified=(Vec2)square.invoke(null,key);entity.travel(new Vec3(modified.x,0,modified.y));
            JsonObject out=new JsonObject();out.add("position",vec(entity.p));out.add("velocity",vec(entity.v));out.addProperty("ground",entity.ground);out.addProperty("delay",entity.delay);ticks.add(out);
          }results.add(ticks);
        }Files.writeString(Path.of(args[1]),new Gson().toJson(results));
    }
}
"#;

fn java_bin() -> PathBuf {
    if let Some(p) =
        std::env::var_os("MINECRAFT_JAVA_HOME").or_else(|| std::env::var_os("JAVA_HOME"))
    {
        return PathBuf::from(p).join("bin");
    }
    PathBuf::from(std::env::var_os("USERPROFILE").expect("set MINECRAFT_JAVA_HOME")).join("AppData/Local/Packages/Microsoft.4297127D64EC6_8wekyb3d8bbwe/LocalCache/Local/runtime/java-runtime-epsilon/windows-x64/java-runtime-epsilon/bin")
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
fn run(cmd: &mut Command) {
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "fluid motion oracle failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
fn vec3(v: &Value) -> DVec3 {
    DVec3::new(
        v[0].as_f64().unwrap(),
        v[1].as_f64().unwrap(),
        v[2].as_f64().unwrap(),
    )
}

#[test]
#[ignore = "requires Minecraft 26.3 and Java 25 JDK"]
fn water_and_lava_motion_match_vanilla_player() {
    let game = std::env::var_os("MINECRAFT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("APPDATA").unwrap()).join(".minecraft"));
    let _verified = native_resources::packs::ResourceStack::open(&game).unwrap();
    let mut scenarios = Vec::new();
    let angles = [0.0_f32, 45.0, -90.0, 113.0, 180.0, -179.9, 12.5, 270.0];
    for s in 0..64 {
        let mut inputs = Vec::new();
        for t in 0..80 {
            let (forward, right) = match (t / 10 + s) % 6 {
                0 => (1.0, 0.0),
                1 => (0.0, 0.0),
                2 => (1.0, 1.0),
                3 => (0.0, 1.0),
                4 => (-1.0, 0.0),
                _ => (1.0, -1.0),
            };
            let water = (s + t / 15) % 3 != 0;
            let swim = water && (s + t / 12) % 3 == 0;
            inputs.push(json!({"forward":forward,"right":right,"yaw":angles[(s+t/20)%angles.len()],"jump":(s+t/9)%3==0,"sprint":(s+t/11)%3==0,"slow":(s+t/13)%4==0,"water":water,"swim":swim,"descend":(s+t/7)%4==0,"look":([-0.8,-0.21,0.0,0.2,0.9][(s+t/8)%5]),"depth":([0.2,0.4,0.8,1.5][(s+t/10)%4]),"free":(s+t/6)%3==0,"above":(s+t/5)%2==0}));
        }
        scenarios.push(json!({"position":[0.0,if s%2==0{0.0}else{2.0},0.0],"velocity":[if s%3==0{0.005}else{0.0},0.0,0.0],"ground":s%2==0,"wall":2.75,"inputs":inputs}));
    }
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("FluidMotionOracle.java");
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
        .arg("FluidMotionOracle")
        .arg(&input)
        .arg(&output));
    let expected: Vec<Value> = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
    let mut errors = Vec::new();
    for (s, (scenario, results)) in scenarios.iter().zip(&expected).enumerate() {
        let mut state = MotionState {
            position: vec3(&scenario["position"]),
            velocity: vec3(&scenario["velocity"]),
            on_ground: scenario["ground"].as_bool().unwrap(),
            jump_delay: 0,
        };
        for (t, (input, result)) in scenario["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .zip(results.as_array().unwrap())
            .enumerate()
        {
            let f = |key: &str| input[key].as_f64().unwrap() as f32;
            let b = |key: &str| input[key].as_bool().unwrap();
            let command = MovementInput {
                forward: f("forward"),
                right: f("right"),
                yaw_degrees: f("yaw"),
                jump: b("jump"),
                sprinting: b("sprint"),
                using_item: false,
                moving_slowly: b("slow"),
            };
            let fluid = FluidMovement {
                kind: if b("water") {
                    FluidKind::Water
                } else {
                    FluidKind::Lava
                },
                depth: input["depth"].as_f64().unwrap(),
                descend: b("descend"),
                swimming: b("swim"),
                look_y: input["look"].as_f64().unwrap(),
                fluid_above: b("above"),
            };
            state.tick_fluid(
                command,
                fluid,
                |position, requested, _| {
                    let mut actual = requested;
                    if actual.y < 0.0 && position.y + actual.y < 0.0 {
                        actual.y = -position.y;
                    }
                    let wall = scenario["wall"].as_f64().unwrap();
                    if actual.x > 0.0 && position.x + f64::from(0.3_f32) + actual.x > wall {
                        actual.x = (wall - position.x - f64::from(0.3_f32)).max(0.0);
                    }
                    (requested, actual)
                },
                |_, _| b("free"),
            );
            if ((state.position - vec3(&result["position"]))
                .abs()
                .max_element()
                > 1.0e-6
                || (state.velocity - vec3(&result["velocity"]))
                    .abs()
                    .max_element()
                    > 1.0e-6
                || state.on_ground != result["ground"].as_bool().unwrap()
                || state.jump_delay != result["delay"].as_u64().unwrap() as u8)
                && errors.len() < 20
            {
                errors.push(format!(
                    "scenario {s}, tick {t}, input {input}: native {state:?}, Java {result}"
                ));
            }
        }
    }
    assert!(
        errors.is_empty(),
        "fluid motion mismatch:\n{}",
        errors.join("\n")
    );
    println!(
        "Compared {} fluid movement ticks with Minecraft 26.3",
        64 * 80
    );
}
