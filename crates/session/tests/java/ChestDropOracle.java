import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.multiplayer.ClientLevel;
import net.minecraft.core.BlockPos;
import net.minecraft.world.Containers;
import net.minecraft.world.SimpleContainer;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.item.*;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.util.RandomSource;
import net.minecraft.core.component.*;

public class ChestDropOracle {
    static class World extends ClientLevel {
        RandomSource seeded; List<ItemEntity> items;
        World() { super(null,null,null,null,0,0,null,false,0L,0); }
        @Override public RandomSource getRandom() { return seeded; }
        @Override public boolean addFreshEntity(Entity entity) { items.add((ItemEntity)entity); return true; }
    }
    static class ServerWorld extends net.minecraft.server.level.ServerLevel {
        RandomSource seeded; List<ItemEntity> items; net.minecraft.world.level.gamerules.GameRules rules;
        ServerWorld() { super(null,null,null,null,null,null,false,0L,null,false); }
        @Override public RandomSource getRandom() { return seeded; }
        @Override public net.minecraft.world.level.gamerules.GameRules getGameRules() { return rules; }
        @Override public int getNextEntityId() { return 7; }
        @Override public boolean addFreshEntity(Entity entity) { items.add((ItemEntity)entity); return true; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var registries=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(net.minecraft.data.registries.VanillaRegistries.createWorldLookup());
        for(var pending:net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var named=new net.minecraft.world.level.block.entity.ChestBlockEntity(BlockPos.ZERO,net.minecraft.world.level.block.Blocks.CHEST.defaultBlockState());
        var name=net.minecraft.world.level.block.entity.BaseContainerBlockEntity.class.getDeclaredField("name"); name.setAccessible(true); name.set(named,net.minecraft.network.chat.Component.literal("treasure"));
        var params=new net.minecraft.world.level.storage.loot.LootParams(null,net.minecraft.util.context.ContextMap.builder().set(net.minecraft.world.level.storage.loot.parameters.LootContextParams.BLOCK_ENTITY,named).build(),Map.of(),0.0f);
        var ctor=net.minecraft.world.level.storage.loot.LootContext.class.getDeclaredConstructor(net.minecraft.world.level.storage.loot.LootParams.class,RandomSource.class,net.minecraft.core.HolderGetter.Provider.class); ctor.setAccessible(true);
        var context=ctor.newInstance(params,new LegacyRandomSource(1),registries);
        var copy=net.minecraft.world.level.storage.loot.functions.CopyComponentsFunction.copyComponentsFromBlockEntity(net.minecraft.world.level.storage.loot.parameters.LootContextParams.BLOCK_ENTITY).include(DataComponents.CUSTOM_NAME).build();
        var copied=copy.apply(new ItemStack(Items.CHEST),context);
        if(!copied.get(DataComponents.CUSTOM_NAME).getString().equals("treasure")) throw new AssertionError("chest custom name copy");
        for(boolean trapped : new boolean[]{false,true}) for(boolean styled : new boolean[]{false,true}) {
            var nameComponent=net.minecraft.network.chat.Component.literal("\ubcf4\uad00\ud568 \u2660");
            if(styled) nameComponent.withStyle(net.minecraft.ChatFormatting.GOLD,net.minecraft.ChatFormatting.BOLD);
            var item=new ItemStack(trapped ? Items.TRAPPED_CHEST : Items.CHEST);
            item.set(DataComponents.CUSTOM_NAME,nameComponent);
            var items=new ArrayList<ItemStack>(Collections.nCopies(30,ItemStack.EMPTY));
            items.set(4,new ItemStack(Items.DIAMOND,3)); items.set(26,new ItemStack(Items.STONE,32)); items.set(29,new ItemStack(Items.DIAMOND,7));
            item.set(DataComponents.CONTAINER,net.minecraft.world.item.component.ItemContainerContents.fromItems(items));
            var block=trapped ? net.minecraft.world.level.block.Blocks.TRAPPED_CHEST : net.minecraft.world.level.block.Blocks.CHEST;
            net.minecraft.world.level.block.entity.ChestBlockEntity placed=trapped
                ? new net.minecraft.world.level.block.entity.TrappedChestBlockEntity(BlockPos.ZERO,block.defaultBlockState())
                : new net.minecraft.world.level.block.entity.ChestBlockEntity(BlockPos.ZERO,block.defaultBlockState());
            placed.applyComponentsFromItemStack(item);
            if(!nameComponent.equals(placed.getCustomName())) throw new AssertionError("placed chest name");
            var tag=placed.saveWithFullMetadata(registries);
            try(var out=new java.io.DataOutputStream(Files.newOutputStream(Path.of(args[0]+"."+trapped+"."+styled+".nbt")))) { net.minecraft.nbt.NbtIo.write(tag,(java.io.DataOutput)out); }
            var itemTag=(net.minecraft.nbt.CompoundTag)ItemStack.CODEC.encodeStart(registries.createSerializationContext(net.minecraft.nbt.NbtOps.INSTANCE),item).getOrThrow();
            try(var out=new java.io.DataOutputStream(Files.newOutputStream(Path.of(args[0]+"."+trapped+"."+styled+".item.nbt")))) { net.minecraft.nbt.NbtIo.write(itemTag,(java.io.DataOutput)out); }
        }
        Items.DIRT.builtInRegistryHolder().bindComponents(DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE,64).build());
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var delay=ItemEntity.class.getDeclaredField("pickupDelay"); delay.setAccessible(true);
        var rows=new JsonArray();
        for(int seed=0;seed<32;seed++) {
            var world=(World)unsafe.allocateInstance(World.class); world.seeded=new LegacyRandomSource(seed); world.items=new ArrayList<>();
            var container=new SimpleContainer(27);
            for(int i=0;i<27;i++) if ((i+seed)%4==0) container.setItem(i,new ItemStack(Items.DIRT,new int[]{1,9,10,11,29,30,31,64}[(i/4+seed)%8]));
            var position=new BlockPos(seed%2==0 ? -16000000 : 16000000,seed-64,-seed*31);
            Containers.dropContents(world,position,container);
            var row=new JsonObject(); row.addProperty("seed",seed); var drops=new JsonArray();
            for(var item:world.items) { var drop=new JsonObject(); drop.addProperty("count",item.getItem().getCount()); drop.addProperty("x",item.getX()); drop.addProperty("y",item.getY()); drop.addProperty("z",item.getZ()); drop.addProperty("vx",item.getDeltaMovement().x); drop.addProperty("vy",item.getDeltaMovement().y); drop.addProperty("vz",item.getDeltaMovement().z); drop.addProperty("delay",delay.getInt(item)); drops.add(drop); }
            row.add("drops",drops); row.addProperty("next",world.seeded.nextDouble());
            net.minecraft.world.level.block.Block.popResource(world,position,new ItemStack(Items.CHEST));
            row.addProperty("blockNext",world.seeded.nextDouble()); rows.add(row);
        }
        Files.writeString(Path.of(args[0]),rows.toString());
        var server=(ServerWorld)unsafe.allocateInstance(ServerWorld.class);
        server.rules=new net.minecraft.world.level.gamerules.GameRules(net.minecraft.world.flag.FeatureFlags.DEFAULT_FLAGS);
        var faces=new JsonArray();
        for(int seed=0;seed<16;seed++) for(var face:net.minecraft.core.Direction.values()) for(boolean enabled:new boolean[]{false,true}) {
            server.seeded=new LegacyRandomSource(seed); server.items=new ArrayList<>();
            server.rules.set(net.minecraft.world.level.gamerules.GameRules.BLOCK_DROPS,enabled,null);
            var pos=new BlockPos(seed%2==0 ? -16000000 : 16000000,seed-64,-seed*31);
            net.minecraft.world.level.block.Block.popResourceFromFace(server,pos,face,new ItemStack(Items.DIRT));
            var row=new JsonObject(); row.addProperty("seed",seed); row.addProperty("face",face.getName()); row.addProperty("enabled",enabled); row.addProperty("next",server.seeded.nextDouble());
            if(!server.items.isEmpty()) { var item=server.items.get(0); row.addProperty("x",item.getX()); row.addProperty("y",item.getY()); row.addProperty("z",item.getZ()); row.addProperty("vx",item.getDeltaMovement().x); row.addProperty("vy",item.getDeltaMovement().y); row.addProperty("vz",item.getDeltaMovement().z); }
            faces.add(row);
        }
        Files.writeString(Path.of(args[0]+".faces.json"),faces.toString());
    }
}
