import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.nio.file.*;
import java.util.*;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.nbt.*;
import net.minecraft.network.chat.Component;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.*;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.*;

public class BannerPickOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var lookup=net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
    var reloadable=net.minecraft.data.registries.VanillaRegistries.createReloadableLookup(lookup);
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
    var registry=lookup.lookupOrThrow(Registries.BANNER_PATTERN);
    var patterns=new BannerPatternLayers(List.of(new BannerPatternLayers.Layer(registry.getOrThrow(BannerPatterns.STRIPE_BOTTOM),DyeColor.RED),new BannerPatternLayers.Layer(registry.getOrThrow(BannerPatterns.CROSS),DyeColor.BLUE)));
    var rows=new JsonArray(); var entities=new CompoundTag();
    for(String color:new String[]{"white","orange","magenta","light_blue","yellow","lime","pink","gray","light_gray","cyan","purple","blue","brown","green","red","black"}) {
      for(String face:new String[]{"standing","north","south","west","east"}) {
        var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+color+(face.equals("standing")?"_banner":"_wall_banner")));
        var state=block.defaultBlockState();
        if(!face.equals("standing")) state=state.setValue(WallBannerBlock.FACING,Direction.byName(face));
        var entity=new BannerBlockEntity(BlockPos.ZERO,state);
        var stack=new ItemStack(BuiltInRegistries.ITEM.getValue(Identifier.parse("minecraft:"+color+"_banner")));
        stack.set(DataComponents.BANNER_PATTERNS,patterns); stack.set(DataComponents.CUSTOM_NAME,Component.literal("Named banner"));
        entity.applyComponentsFromItemStack(stack);
        var saved=entity.saveWithFullMetadata(lookup); entities.put(color+"_"+face,saved);
        var picked=entity.getItem(); var row=new JsonObject();
        row.addProperty("color",color); row.addProperty("face",face);
        row.addProperty("id",BuiltInRegistries.ITEM.getKey(picked.getItem()).toString());
        row.addProperty("max",picked.getMaxStackSize());
        row.add("patterns",BannerPatternLayers.CODEC.encodeStart(lookup.createSerializationContext(JsonOps.INSTANCE),picked.get(DataComponents.BANNER_PATTERNS)).getOrThrow());
        var shape=new JsonArray(); for(var b:state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO).toAabbs()) { var a=new JsonArray(); for(double d:new double[]{b.minX,b.minY,b.minZ,b.maxX,b.maxY,b.maxZ}) a.add(d); shape.add(a); }
        row.add("outline",shape); row.addProperty("collision_empty",state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO).isEmpty()); rows.add(row);
      }
    }
    NbtIo.write(entities,Path.of(args[0])); Files.writeString(Path.of(args[1]),rows.toString());
  }
}
