import com.google.gson.*;
import java.lang.reflect.Field;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.core.*;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.world.damagesource.*;
import net.minecraft.world.entity.*;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.tags.TagKey;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.GameType;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.Vec3;

public class PlayerLandingSoundOracle {
    static class World extends ServerLevel {
        BlockState below;
        DamageSources sources;
        List<String> events;
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public BlockState getBlockState(BlockPos pos) { return below; }
        @Override public DamageSources damageSources() { return sources; }
        @Override public void broadcastEntityEvent(Entity entity, byte event) { }
    }
    static class TestPlayer extends Player {
        World world; boolean silent;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean is(TagKey<EntityType<?>> tag) { return false; }
        @Override public boolean isVehicle() { return false; }
        @Override public boolean isIgnoringFallDamageFromCurrentImpulse() { return false; }
        @Override public boolean isSilent() { return silent; }
        @Override public double getAttributeValue(Holder<Attribute> attr) {
            if (attr == Attributes.SAFE_FALL_DISTANCE) return 3.0;
            if (attr == Attributes.FALL_DAMAGE_MULTIPLIER) return 1.0;
            throw new IllegalStateException();
        }
        @Override public void awardStat(Identifier id,int amount) { }
        @Override public void playSound(SoundEvent sound,float volume,float pitch) {
            world.events.add(sound.location().toString()+":"+volume+":"+pitch);
        }
        @Override public boolean hurtServer(ServerLevel level,DamageSource source,float amount) { return false; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(World)unsafe.allocateInstance(World.class);
        world.sources=(DamageSources)unsafe.allocateInstance(DamageSources.class);
        var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class); player.world=world;
        Field level=Entity.class.getDeclaredField("level"); level.setAccessible(true); level.set(player,world);
        Field pos=Entity.class.getDeclaredField("position"); pos.setAccessible(true); pos.set(player,new Vec3(8.5,2.0,8.5));
        Field abilities=Player.class.getDeclaredField("abilities"); abilities.setAccessible(true); abilities.set(player,new Abilities());
        var blocks=new LinkedHashMap<String,Block>();
        blocks.put("stone",Blocks.STONE); blocks.put("dirt",Blocks.DIRT); blocks.put("grass_block",Blocks.GRASS_BLOCK);
        blocks.put("sand",Blocks.SAND); blocks.put("gravel",Blocks.GRAVEL); blocks.put("oak_planks",Blocks.OAK_PLANKS);
        blocks.put("hay_block",Blocks.HAY_BLOCK); blocks.put("honey_block",Blocks.HONEY_BLOCK);
        blocks.put("cobblestone",Blocks.COBBLESTONE); blocks.put("stone_bricks",Blocks.STONE_BRICKS);
        blocks.put("granite",Blocks.GRANITE); blocks.put("diorite",Blocks.DIORITE); blocks.put("andesite",Blocks.ANDESITE);
        blocks.put("deepslate",Blocks.DEEPSLATE); blocks.put("cobbled_deepslate",Blocks.COBBLED_DEEPSLATE);
        blocks.put("netherrack",Blocks.NETHERRACK); blocks.put("end_stone",Blocks.END_STONE);
        blocks.put("oak_log",Blocks.OAK_LOG); blocks.put("snow_block",Blocks.SNOW_BLOCK); blocks.put("ice",Blocks.ICE);
        blocks.put("sandstone",Blocks.SANDSTONE); blocks.put("terracotta",Blocks.TERRACOTTA); blocks.put("moss_block",Blocks.MOSS_BLOCK);
        blocks.put("red_sand",Blocks.RED_SAND); blocks.put("red_sandstone",Blocks.RED_SANDSTONE);
        blocks.put("nether_bricks",Blocks.NETHER_BRICKS); blocks.put("blackstone",Blocks.BLACKSTONE);
        blocks.put("basalt",Blocks.BASALT); blocks.put("obsidian",Blocks.OBSIDIAN); blocks.put("glass",Blocks.GLASS);
        var rows=new JsonArray();
        for(var entry:blocks.entrySet()) for(boolean silent:new boolean[]{false,true})
            for(double distance:new double[]{3.0,4.0,7.0,8.0,24.0}) {
                world.below=entry.getValue().defaultBlockState(); world.events=new ArrayList<>(); player.silent=silent;
                entry.getValue().fallOn(world,world.below,BlockPos.ZERO,player,distance);
                var row=new JsonObject(); row.addProperty("block",entry.getKey()); row.addProperty("silent",silent);
                row.addProperty("distance",distance); row.add("events",new Gson().toJsonTree(world.events)); rows.add(row);
                var boxes=new JsonArray();
                for(var box:world.below.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO).toAabbs()) {
                    var a=new JsonArray(); for(double v:new double[]{box.minX,box.minY,box.minZ,box.maxX,box.maxY,box.maxZ}) a.add(v); boxes.add(a);
                }
                row.add("boxes",boxes);
            }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
