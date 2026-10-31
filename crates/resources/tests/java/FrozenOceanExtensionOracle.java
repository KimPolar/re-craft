import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.BlockColumn;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.material.MaterialSystem;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class FrozenOceanExtensionOracle {
  static void set(Object o,String name,Object value)throws Exception {
    var f=o.getClass().getDeclaredField(name);f.setAccessible(true);f.set(o,value);
  }
  public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var defs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var method=MaterialSystem.class.getDeclaredMethod("frozenOceanExtension",int.class,Biome.class,BlockColumn.class,BlockPos.MutableBlockPos.class,int.class,int.class,int.class);
    method.setAccessible(true);
    String[] names={"iceberg_surface","iceberg_pillar","iceberg_pillar_roof"};
    String[] fields={"icebergSurfaceNoise","icebergPillarNoise","icebergPillarRoofNoise"};
    for(int mode=0;mode<2;mode++) for(long seed:new long[]{0,2345,-1}) {
      RandomSource random=mode==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
      var root=random.forkPositional();
      var system=(MaterialSystem)unsafe.allocateInstance(MaterialSystem.class);
      set(system,"seaLevel",63);set(system,"noiseRandom",root);
      for(int i=0;i<3;i++) {
        var id=Identifier.withDefaultNamespace(names[i]);
        var params=NormalNoise.DIRECT_CODEC.parse(JsonOps.INSTANCE,defs.get("minecraft:"+names[i])).getOrThrow();
        set(system,fields[i],params.create(root.fromHashOf(id)));
      }
      out.printf("S %d %d%n",mode,seed);
      for(int n=0;n<1000;n++) for(int variant=0;variant<3;variant++) {
        int x=n*31-15000,z=n*n*7-10000,floor=45+n%4;
        float base=n%3==0?0.0f:n%3==1?0.8f:0.1f;
        boolean frozen=(n&1)!=0;
        var biome=new Biome.BiomeBuilder().temperature(base).downfall(0.5f).hasPrecipitation(false)
          .temperatureAdjustment(frozen?Biome.TemperatureModifier.FROZEN:Biome.TemperatureModifier.NONE)
          .specialEffects(new BiomeSpecialEffects.Builder().waterColor(0).build())
          .generationSettings(BiomeGenerationSettings.EMPTY).build();
        var blocks=new BlockState[160];
        for(int i=0;i<blocks.length;i++) {
          int y=i-32;
          blocks[i]=(y<=floor?Blocks.STONE:y<63?Blocks.WATER:Blocks.AIR).defaultBlockState();
        }
        if(variant==1) blocks[75+32]=Blocks.DIRT.defaultBlockState();
        if(variant==2) { blocks[55+32]=Blocks.AIR.defaultBlockState(); blocks[60+32]=Blocks.AIR.defaultBlockState(); }
        int start=variant==1?76:63;
        BlockColumn column=new BlockColumn() {
          public BlockState getBlock(int y){return y< -32 || y>=128?Blocks.AIR.defaultBlockState():blocks[y+32];}
          public void setBlock(int y,BlockState b){if(y>=-32&&y<128)blocks[y+32]=b;}
        };
        method.invoke(system,28+n%7,biome,column,new BlockPos.MutableBlockPos(),x,z,start);
        var result=new StringBuilder();
        for(var block:blocks) result.append(block.isAir()?'a':block.is(Blocks.STONE)?'s':block.is(Blocks.WATER)?'w':block.is(Blocks.DIRT)?'d':block.is(Blocks.SNOW_BLOCK)?'n':'i');
        out.printf("P %d %d %d %d %s %s%n",n,x,z,variant,biome.shouldMeltFrozenOceanIcebergSlightly(new BlockPos(x,63,z),63),result);
      }
    }
  }
}
