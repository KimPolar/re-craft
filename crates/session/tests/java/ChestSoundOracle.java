import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.sounds.SoundEvent;
import net.minecraft.sounds.SoundSource;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.ChestBlock;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.world.level.levelgen.LegacyRandomSource;

public class ChestSoundOracle {
    static class World extends ClientLevel {
        RandomSource rng; JsonArray captured;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public RandomSource getRandom() { return rng; }
        @Override public void playSound(Entity source, double x, double y, double z, SoundEvent sound, SoundSource category, float volume, float pitch) {
            var row=new JsonArray(); row.add(x); row.add(y); row.add(z); row.add(sound.location().toString()); row.add(category.getName()); row.add(volume); row.add(pitch); captured.add(row);
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var world=(World)((sun.misc.Unsafe)field.get(null)).allocateInstance(World.class);
        world.rng=new LegacyRandomSource(123); world.captured=new JsonArray();
        var play=ChestBlockEntity.class.getDeclaredMethod("playSound", net.minecraft.world.level.Level.class, BlockPos.class, net.minecraft.world.level.block.state.BlockState.class, SoundEvent.class);
        play.setAccessible(true);
        for (var block : new ChestBlock[]{(ChestBlock)Blocks.CHEST,(ChestBlock)Blocks.TRAPPED_CHEST})
            for (var type : new ChestType[]{ChestType.SINGLE,ChestType.LEFT,ChestType.RIGHT})
                for (var facing : new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST})
                    for (boolean opening : new boolean[]{true,false}) {
                        var state=block.defaultBlockState().setValue(ChestBlock.TYPE,type).setValue(ChestBlock.FACING,facing);
                        play.invoke(null,world,new BlockPos(16000008,2,-15999994),state,opening ? block.getOpenChestSound() : block.getCloseChestSound());
                    }
        Files.writeString(Path.of(args[0]),world.captured.toString());
    }
}
