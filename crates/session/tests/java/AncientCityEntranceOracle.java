import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.io.InputStreamReader;
import java.lang.reflect.Proxy;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.HolderSet;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class AncientCityEntranceOracle {
    static JsonObject json(ZipFile jar, String path) throws Exception {
        var entry = jar.getEntry(path);
        if (entry == null) throw new AssertionError("missing " + path);
        try (var reader = new InputStreamReader(jar.getInputStream(entry), java.nio.charset.StandardCharsets.UTF_8)) {
            return JsonParser.parseReader(reader).getAsJsonObject();
        }
    }
    static HolderSet<Block> tag(ZipFile jar, String name) throws Exception {
        var holders = new ArrayList<net.minecraft.core.Holder<Block>>();
        for (var value : json(jar, "data/minecraft/tags/block/" + name + ".json").getAsJsonArray("values")) {
            var id = value.getAsString();
            if (id.startsWith("#")) throw new AssertionError("nested tag requires explicit handling: " + id);
            var block = BuiltInRegistries.BLOCK.getOptional(Identifier.parse(id)).orElseThrow();
            holders.add(block.builtInRegistryHolder());
        }
        return HolderSet.direct(holders);
    }
    public static void main(String[] args) throws Exception {
        var out=System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        try (var jar=new ZipFile(args[0])) {
            var template=new StructureTemplate();
            var templateName=args.length>4 ? args[4] : "city/entrance/entrance_connector";
            var processorName=args.length>5 ? args[5] : "ancient_city_generic_degradation";
            var entry=jar.getEntry("data/minecraft/structure/ancient_city/"+templateName+".nbt");
            if(entry==null)throw new AssertionError("missing template " + templateName);
            try(var input=jar.getInputStream(entry)) {
                var nbt=NbtIo.readCompressed(input,NbtAccounter.unlimitedHeap());
                if(!nbt.getListOrEmpty("entities").isEmpty())throw new AssertionError("template entities need comparison");
                for(var value:nbt.getListOrEmpty("blocks")) {
                    var data=((net.minecraft.nbt.CompoundTag)value).getCompoundOrEmpty("nbt");
                    if(!data.isEmpty() && !data.getStringOr("id","").equals("minecraft:jigsaw"))
                        throw new AssertionError("template block entities need comparison: "+data);
                }
                template.load(BuiltInRegistries.BLOCK,nbt);
            }
            var states=new HashMap<BlockPos,BlockState>();
            var world=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                new Class<?>[]{ServerLevelAccessor.class},(proxy,method,arguments)->switch(method.getName()) {
                    case "holderLookup" -> BuiltInRegistries.BLOCK;
                    case "getBlockState" -> states.getOrDefault((BlockPos)arguments[0],
                        ((BlockPos)arguments[0]).getY()<8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
                    case "setBlock" -> {states.put(((BlockPos)arguments[0]).immutable(),(BlockState)arguments[1]);yield true;}
                    case "getBlockEntity" -> null;
                    default -> throw new UnsupportedOperationException(method.toString());
                });
            var config=json(jar,"data/minecraft/worldgen/processor_list/"+processorName+".json").getAsJsonArray("processors");
            if(config.size()!=3)throw new AssertionError("unexpected processor count");
            var rot=config.get(0).getAsJsonObject();
            var rules=config.get(1).getAsJsonObject();
            var protectedBlocks=config.get(2).getAsJsonObject();
            if(!rot.get("processor_type").getAsString().equals("minecraft:block_rot")
                || !rot.get("rottable_blocks").getAsString().equals("#minecraft:ancient_city_replaceable")
                || !rules.get("processor_type").getAsString().equals("minecraft:rule")
                || !protectedBlocks.get("processor_type").getAsString().equals("minecraft:protected_blocks")
                || !protectedBlocks.get("value").getAsString().equals("#minecraft:features_cannot_replace"))
                throw new AssertionError("unexpected processor configuration");
            var replacements=new ArrayList<ProcessorRule>();
            for(var value:rules.getAsJsonArray("rules")) {
                var rule=value.getAsJsonObject();var input=rule.getAsJsonObject("input_predicate");
                if(!input.get("predicate_type").getAsString().equals("minecraft:random_block_match")
                    || !rule.getAsJsonObject("location_predicate").get("predicate_type").getAsString().equals("minecraft:always_true"))
                    throw new AssertionError("unexpected rule");
                var source=BuiltInRegistries.BLOCK.getOptional(Identifier.parse(input.get("block").getAsString())).orElseThrow();
                var replacement=BuiltInRegistries.BLOCK.getOptional(Identifier.parse(rule.get("output_state").getAsString())).orElseThrow();
                replacements.add(new ProcessorRule(new RandomBlockMatchTest(source,input.get("probability").getAsFloat()),AlwaysTrueTest.INSTANCE,replacement.defaultBlockState()));
            }
            var settings=new StructurePlaceSettings().setBoundingBox(new BoundingBox(0,0,0,15,15,15))
                .setRotation(Rotation.valueOf(args[3]))
                .setKnownShape(true).setIgnoreEntities(true).setLiquidSettings(LiquidSettings.IGNORE_WATERLOGGING)
                .addProcessor(BlockIgnoreProcessor.STRUCTURE_BLOCK).addProcessor(JigsawReplacementProcessor.INSTANCE)
                .addProcessor(new BlockRotProcessor(tag(jar,"ancient_city_replaceable"),rot.get("integrity").getAsFloat()))
                .addProcessor(new RuleProcessor(replacements)).addProcessor(new ProtectedBlockProcessor(tag(jar,"features_cannot_replace")));
            var random=new WorldgenRandom(new XoroshiroRandomSource(42));
            var origin=new BlockPos(Integer.parseInt(args[1]),0,Integer.parseInt(args[2]));
            if(!template.placeInWorld(world,origin,origin,settings,random,18))throw new AssertionError("template was not placed");
            for(int y=0;y<16;y++)for(int z=0;z<16;z++)for(int x=0;x<16;x++) {
                var pos=new BlockPos(x,y,z);var state=states.getOrDefault(pos,y<8 ? Blocks.STONE.defaultBlockState() : Blocks.AIR.defaultBlockState());
                var properties=new JsonObject();state.getValues().forEach(value->properties.addProperty(value.property().getName(),value.valueName()));
                out.printf("BLOCK\t%d\t%d\t%d\t%s\t%s%n",x,y,z,BuiltInRegistries.BLOCK.getKey(state.getBlock()),properties);
            }
            out.printf("RANDOM\t%d%n",random.nextLong());
        }
    }
}
