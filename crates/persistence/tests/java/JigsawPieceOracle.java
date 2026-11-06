import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.lang.reflect.Field;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.*;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;
import net.minecraft.world.level.levelgen.structure.pools.*;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class JigsawPieceOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var vanilla=VanillaRegistries.createWorldLookup();
        var feature=vanilla.lookupOrThrow(Registries.PLACED_FEATURE).listElements().findFirst().orElseThrow().key().identifier().toString();
        var single=JsonParser.parseString("{\"element_type\":\"minecraft:single_pool_element\",\"location\":\"example:room\",\"processors\":\"minecraft:empty\",\"projection\":\"rigid\",\"override_liquid_settings\":\"ignore_waterlogging\"}").getAsJsonObject();
        var legacy=single.deepCopy(); legacy.addProperty("element_type","minecraft:legacy_single_pool_element"); legacy.remove("override_liquid_settings");
        var feat=new JsonObject(); feat.addProperty("element_type","minecraft:feature_pool_element"); feat.addProperty("feature",feature); feat.addProperty("projection","terrain_matching");
        var nested=new JsonObject(); nested.addProperty("element_type","minecraft:list_pool_element"); nested.addProperty("projection","terrain_matching"); var members=new JsonArray(); members.add(single.deepCopy()); members.add(feat.deepCopy()); nested.add("elements",members);
        var inputs=new JsonObject[]{single,legacy,feat,nested};
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true); var unsafe=(sun.misc.Unsafe)uf.get(null);
        var manager=(StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        RegistryAccess access=new RegistryAccess() {
            public <E> Optional<Registry<E>> lookup(ResourceKey<? extends Registry<? extends E>> key) { return Optional.empty(); }
            public java.util.stream.Stream<RegistryAccess.RegistryEntry<?>> registries() { return java.util.stream.Stream.empty(); }
            public <T> RegistryOps<T> createSerializationContext(com.mojang.serialization.DynamicOps<T> ops) { return vanilla.createSerializationContext(ops); }
        };
        var context=new StructurePieceSerializationContext(null,access,manager);
        for(int id=0;id<inputs.length;id++) {
            out.printf("D\t%d\t%s%n",id,inputs[id]);
            var element=StructurePoolElement.CODEC.parse(vanilla.createSerializationContext(JsonOps.INSTANCE),inputs[id]).getOrThrow();
            for(var rotation:Rotation.values()) for(var liquid:LiquidSettings.values()) {
                var piece=new PoolElementStructurePiece(manager,element,new BlockPos(1,2,-3),4,rotation,new BoundingBox(-2,2,-3,5,8,9),liquid);
                piece.addJunction(new JigsawJunction(12,-5,7,-2,StructureTemplatePool.Projection.RIGID));
                piece.addJunction(new JigsawJunction(-4,60,2,3,StructureTemplatePool.Projection.TERRAIN_MATCHING));
                String filename=id+"_"+rotation.ordinal()+"_"+liquid.ordinal()+".nbt";
                NbtIo.write(piece.createTag(context),Path.of(args[0],filename));
                out.printf("T\t%d\t%d\t%s\t%s%n",id,rotation.ordinal(),liquid.getSerializedName(),filename);
            }
        }
    }
}
