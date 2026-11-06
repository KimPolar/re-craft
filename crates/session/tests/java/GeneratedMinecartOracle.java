import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.vehicle.minecart.MinecartChest;
import net.minecraft.world.level.storage.TagValueOutput;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.loot.BuiltInLootTables;

public class GeneratedMinecartOracle {
    static boolean experimental;
    static class World extends ServerLevel {
        World(){super(null,null,null,null,null,null,false,0L,List.of(),false);}
        @Override public int getNextEntityId(){return 1;}
        @Override public net.minecraft.world.flag.FeatureFlagSet enabledFeatures(){
            var flags=net.minecraft.world.flag.FeatureFlags.VANILLA_SET;
            return experimental?flags.join(net.minecraft.world.flag.FeatureFlagSet.of(net.minecraft.world.flag.FeatureFlags.MINECART_IMPROVEMENTS)):flags;
        }
    }
    public static void main(String[] args)throws Exception{
        var out=System.out;var err=System.err;
        try{
            SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
            var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
            var world=(World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
            var lookup=VanillaRegistries.createWorldLookup();
            for(int e=0;e<2;e++)for(int i=0;i<4;i++){
                experimental=e!=0;
                var cart=new MinecartChest(EntityTypes.CHEST_MINECART,world);
                net.minecraft.nbt.CompoundTag original=null;
                if(args[1].equals("rust")) {
                    original=NbtIo.read(Path.of(args[0]).resolve("rust-"+e+"-"+i+".nbt"));
                    cart.load(TagValueInput.create(ProblemReporter.DISCARDING,lookup,original));
                } else {
                    cart.setUUID(new UUID(1,2));
                    cart.setInitialPos(-17.5+i,63.5-i,31.5+i);
                    cart.setLootTable(BuiltInLootTables.ABANDONED_MINESHAFT,new long[]{0,1,-1,Long.MIN_VALUE}[i]);
                }
                var output=TagValueOutput.createWithContext(ProblemReporter.DISCARDING,lookup);
                if(!cart.save(output))throw new AssertionError("minecart not saved");
                var tag=output.buildResult();
                if(original!=null && !original.equals(tag))throw new AssertionError("roundtrip differs "+original+" "+tag);
                if(original==null)NbtIo.write(tag,Path.of(args[0]).resolve("minecart-"+e+"-"+i+".nbt"));
                out.println("T "+e+" "+i+" "+tag);
            }
        }catch(Throwable error){error.printStackTrace(err);System.exit(1);}
    }
}
