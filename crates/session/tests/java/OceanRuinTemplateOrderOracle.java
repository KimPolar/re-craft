import com.google.gson.JsonObject;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

public class OceanRuinTemplateOrderOracle {
    public static void main(String[] args) throws Exception {
        var output=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var palettesField=StructureTemplate.class.getDeclaredField("palettes");
        palettesField.setAccessible(true);
        for (String name : args) {
            var path="data/minecraft/structure/"+name+".nbt";
            try (var stream=OceanRuinTemplateOrderOracle.class.getClassLoader().getResourceAsStream(path)) {
                if (stream==null) throw new AssertionError("missing template "+name);
                var tag=NbtIo.readCompressed(stream,NbtAccounter.unlimitedHeap());
                var template=new StructureTemplate();
                template.load(BuiltInRegistries.BLOCK,tag);
                var palettes=(List<StructureTemplate.Palette>)palettesField.get(template);
                if (palettes.size()!=1) throw new AssertionError("unexpected palette count "+name);
                var blocks=palettes.get(0).blocks();
                output.println("T\t"+name+"\t"+blocks.size());
                for (var block : blocks) {
                    var row=new JsonObject();var pos=block.pos();
                    row.addProperty("x",pos.getX());row.addProperty("y",pos.getY());row.addProperty("z",pos.getZ());
                    row.addProperty("id",BuiltInRegistries.BLOCK.getKey(block.state().getBlock()).toString());
                    row.addProperty("nbt",block.nbt()!=null);
                    var props=new JsonObject();
                    block.state().getValues().forEach(v->props.addProperty(v.property().getName(),v.valueName()));
                    row.add("properties",props);
                    output.println("B\t"+row);
                }
            }
        }
    }
}
