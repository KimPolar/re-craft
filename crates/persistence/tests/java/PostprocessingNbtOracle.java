import java.lang.reflect.Method;
import java.nio.file.Path;
import it.unimi.dsi.fastutil.shorts.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.nbt.*;
import net.minecraft.world.level.chunk.storage.SerializableChunkData;

public class PostprocessingNbtOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    ShortList[] offsets={new ShortArrayList(new short[]{Short.MIN_VALUE,-1,0,0xf0f,0xf0f}),
      null,new ShortArrayList(new short[]{Short.MAX_VALUE,255}),new ShortArrayList()};
    Method pack=SerializableChunkData.class.getDeclaredMethod("packOffsets",ShortList[].class);
    pack.setAccessible(true);
    var expected=(ListTag)pack.invoke(null,(Object)offsets);
    var root=new CompoundTag();root.put("PostProcessing",expected);root.putInt("unrelated",17);
    NbtIo.write(root,Path.of(args[0]));
    var fromRust=NbtIo.read(Path.of(args[1]));
    if(!expected.equals(fromRust.get("PostProcessing")) || fromRust.getIntOr("unrelated",0)!=17)
      throw new AssertionError("Rust postprocessing NBT differs from Java packOffsets");
    out.println("OK");
  }
}
