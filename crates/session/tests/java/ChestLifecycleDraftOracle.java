import java.nio.file.Path;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.*;

public class ChestLifecycleDraftOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registry=VanillaRegistries.createWorldLookup();
        for(var init:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(VanillaRegistries.createReloadableLookup(registry))) init.apply();
        var created=NbtIo.read(Path.of(args[0]));
        if(!(created.get("block_entities") instanceof ListTag entities) || entities.size()!=1) throw new AssertionError("created chest missing");
        var tag=entities.getCompoundOrEmpty(0);
        var pos=new BlockPos(tag.getInt("x").orElseThrow(),tag.getInt("y").orElseThrow(),tag.getInt("z").orElseThrow());
        var block=BlockEntity.loadStatic(pos,Blocks.CHEST.defaultBlockState(),tag,registry);
        if(!(block instanceof ChestBlockEntity chest)) throw new AssertionError("created chest failed to load");
        var stack=chest.getItem(2);
        if(!stack.is(Items.DIAMOND) || stack.getCount()!=3) throw new AssertionError("created chest contents");
        if(chest.getCustomName()==null || !chest.getCustomName().getString().equals("Test chest")) throw new AssertionError("created chest name");
        var removed=NbtIo.read(Path.of(args[1]));
        if(!(removed.get("block_entities") instanceof ListTag remaining) || !remaining.isEmpty()) throw new AssertionError("removed chest remains");
    }
}
