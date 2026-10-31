import java.nio.file.*;
import java.lang.reflect.Field;
import java.util.*;
import com.google.gson.*;
import net.minecraft.client.Minecraft;
import net.minecraft.client.Options;
import net.minecraft.client.OptionInstance;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.*;
import net.minecraft.world.level.biome.Biome;

public class BiomeBlendOracle {
  static class World extends ClientLevel {
    Holder<Biome> biome;
    World() { super(null,null,null,null,0,0,null,false,0L,0); }
    @Override public Holder<Biome> getBiome(BlockPos pos) { return biome; }
  }
  static Field field(Class<?> type,String name) throws Exception { var f=type.getDeclaredField(name); f.setAccessible(true); return f; }
  static int color(int x,int z) { return (((x*37+z*17)&255)<<16) | (((x*11-z*29)&255)<<8) | ((x*7+z*13)&255); }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion();
    net.minecraft.server.Bootstrap.bootStrap();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var minecraft=(Minecraft)unsafe.allocateInstance(Minecraft.class); var options=(Options)unsafe.allocateInstance(Options.class); var setting=(OptionInstance<?>)unsafe.allocateInstance(OptionInstance.class);
    field(Minecraft.class,"instance").set(null,minecraft);
    unsafe.putObject(minecraft,unsafe.objectFieldOffset(field(Minecraft.class,"options")),options);
    unsafe.putObject(options,unsafe.objectFieldOffset(field(Options.class,"biomeBlendRadius")),setting);
    var world=(World)unsafe.allocateInstance(World.class); world.biome=Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
    var output=new JsonArray(); var random=new Random(90210);
    for(int radius=0;radius<=7;radius++) {
      field(OptionInstance.class,"value").set(setting,radius);
      for(int i=0;i<80;i++) {
        int x=random.nextInt(60000001)-30000000,z=random.nextInt(60000001)-30000000,y=random.nextInt(384)-64;
        int result=world.calculateBlockTint(new BlockPos(x,y,z),(biome,bx,bz)->color((int)bx,(int)bz));
        var row=new JsonArray(); row.add(radius); row.add(x); row.add(y); row.add(z); row.add(result); output.add(row);
      }
    }
    Files.writeString(Path.of(args[0]),new Gson().toJson(output));
  }
}
