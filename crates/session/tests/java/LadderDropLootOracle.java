import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.context.ContextMap;
import net.minecraft.world.RandomSequence;
import net.minecraft.world.item.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.storage.loot.*;
import net.minecraft.world.level.storage.loot.parameters.LootContextParams;

public class LadderDropLootOracle {
    static class World extends net.minecraft.client.multiplayer.ClientLevel {
        net.minecraft.util.RandomSource seeded;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public net.minecraft.util.RandomSource getRandom() { return seeded; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var ops=RegistryOps.create(JsonOps.INSTANCE,registries);
        var table=LootTable.DIRECT_CODEC.parse(ops,JsonParser.parseString(Files.readString(Path.of(args[0])))).getOrThrow();
        var ctor=LootContext.class.getDeclaredConstructors()[0]; ctor.setAccessible(true);
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var result=new JsonArray();
        boolean lichen=args.length>2 && args[2].equals("glow_lichen");
        boolean seagrass=args.length>2 && args[2].equals("seagrass");
        boolean tall=args.length>2 && args[2].equals("tall_seagrass");
        if(lichen && Items.GLOW_LICHEN.getDefaultMaxStackSize()!=64)throw new AssertionError("lichen stack limit differs");
        var sequence=Identifier.parse(tall?"minecraft:blocks/tall_seagrass":seagrass?"minecraft:blocks/seagrass":lichen?"minecraft:blocks/glow_lichen":"minecraft:blocks/ladder");
        for(long seed:new long[]{0,1,42,-17,123456789}) for(int step=0;step<8;step++) {
            var random=new RandomSequence(seed,sequence).random();
            var world=(World)unsafe.allocateInstance(World.class);
            world.seeded=new net.minecraft.world.level.levelgen.LegacyRandomSource(9182);
            List<ItemStack> items=List.of();
            for(int n=0;n<=step;n++) {
                var block=tall ? Blocks.TALL_SEAGRASS.defaultBlockState() : seagrass ? Blocks.SEAGRASS.defaultBlockState() : lichen
                    ? Blocks.GLOW_LICHEN.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,n%2==0).setValue(MultifaceBlock.getFaceProperty(Direction.EAST),true)
                    : Blocks.LADDER.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,n%2==0);
                var params=new LootParams(null,ContextMap.builder().set(LootContextParams.TOOL,ItemStack.EMPTY).set(LootContextParams.BLOCK_STATE,block).build(),Map.of(),0.0f);
                var context=(LootContext)ctor.newInstance(params,random,registries);
                var next=new ArrayList<ItemStack>(); table.getRandomItemsRaw(context,next::add); items=next;
                for(var item:items) if(!item.isEmpty()) Block.popResource(world,BlockPos.ZERO,item);
            }
            var drops=new JsonArray(); for(var item:items) if(!item.isEmpty()) {
                var entry=new JsonArray(); entry.add(BuiltInRegistries.ITEM.getKey(item.getItem()).toString()); entry.add(item.getCount()); drops.add(entry);
            }
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("step",step); row.add("drops",drops);
            row.addProperty("next",random.nextLong()); row.addProperty("levelNext",world.seeded.nextDouble()); result.add(row);
        }
        Files.writeString(Path.of(args[1]),result.toString());
    }
}
