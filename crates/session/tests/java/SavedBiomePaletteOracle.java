import com.mojang.serialization.Codec;
import java.nio.file.Path;
import net.minecraft.core.IdMapper;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtOps;
import net.minecraft.world.level.chunk.PalettedContainer;
import net.minecraft.world.level.chunk.Strategy;

public class SavedBiomePaletteOracle {
    public static void main(String[] args) throws Exception {
        var ids = new IdMapper<String>();
        for (String id : new String[] {"minecraft:plains", "minecraft:forest", "minecraft:desert", "minecraft:swamp", "minecraft:ocean"}) ids.add(id);
        var codec = PalettedContainer.codecRW(Codec.STRING, Strategy.createForBiomes(ids), "minecraft:plains");
        for (String file : args) {
            var container = codec.parse(NbtOps.INSTANCE, NbtIo.read(Path.of(file))).getOrThrow();
            for (int y=0; y<4; y++) for (int z=0; z<4; z++) for (int x=0; x<4; x++)
                System.out.println(container.get(x, y, z));
        }
    }
}
