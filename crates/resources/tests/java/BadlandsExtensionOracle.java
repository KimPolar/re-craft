import com.google.gson.JsonParser;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.BlockColumn;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.material.MaterialSystem;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

public class BadlandsExtensionOracle {
  static void set(Object o,String name,Object value)throws Exception {
    var f=o.getClass().getDeclaredField(name);f.setAccessible(true);f.set(o,value);
  }
  public static void main(String[] args)throws Exception {
    var out=System.out; SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var defs=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonObject();
    var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe");field.setAccessible(true);
    var unsafe=(sun.misc.Unsafe)field.get(null);
    var method=MaterialSystem.class.getDeclaredMethod("erodedBadlandsExtension",BlockColumn.class,int.class,int.class,int.class,LevelHeightAccessor.class);
    method.setAccessible(true);
    String[] names={"badlands_surface","badlands_pillar","badlands_pillar_roof"};
    String[] fields={"badlandsSurfaceNoise","badlandsPillarNoise","badlandsPillarRoofNoise"};
    for(int mode=0;mode<2;mode++) for(long seed:new long[]{0,2345,-1}) {
      RandomSource random=mode==0?new LegacyRandomSource(seed):new XoroshiroRandomSource(seed);
      var root=random.forkPositional();
      var system=(MaterialSystem)unsafe.allocateInstance(MaterialSystem.class);
      set(system,"defaultBlock",Blocks.STONE.defaultBlockState());
      for(int i=0;i<3;i++) {
        var id=Identifier.withDefaultNamespace(names[i]);
        var params=NormalNoise.DIRECT_CODEC.parse(JsonOps.INSTANCE,defs.get("minecraft:"+names[i])).getOrThrow();
        set(system,fields[i],params.create(root.fromHashOf(id)));
      }
      out.printf("S %d %d%n",mode,seed);
      for(int n=0;n<300;n++) for(int variant=0;variant<3;variant++) {
        int x=n*31-4500,z=n*n*7-10000,top=48+n%14;
        var blocks=new BlockState[128];
        for(int i=0;i<blocks.length;i++) blocks[i]=(i-32<=top?Blocks.STONE:Blocks.AIR).defaultBlockState();
        if(variant==1) blocks[top+1+32]=Blocks.WATER.defaultBlockState();
        if(variant==2) blocks[top+6+32]=Blocks.DIRT.defaultBlockState();
        int start=top+(variant==1?2:variant==2?7:1);
        BlockColumn column=new BlockColumn() {
          public BlockState getBlock(int y){return y< -32 || y>=96?Blocks.AIR.defaultBlockState():blocks[y+32];}
          public void setBlock(int y,BlockState b){if(y>=-32&&y<96)blocks[y+32]=b;}
        };
        method.invoke(system,column,x,z,start,LevelHeightAccessor.create(-32,128));
        var result=new StringBuilder();
        for(var block:blocks) result.append(block.isAir()?'a':block.is(Blocks.STONE)?'s':block.is(Blocks.WATER)?'w':'d');
        out.printf("P %d %d %d %d %s%n",n,x,z,variant,result);
      }
    }
  }
}
