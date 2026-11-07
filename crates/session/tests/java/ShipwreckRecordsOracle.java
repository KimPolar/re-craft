import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.*;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ShipwreckRecordsOracle {
  @SuppressWarnings({"rawtypes","unchecked"})
  static String key(BlockState state) {
    var props=new ArrayList<String>();
    state.getValues().forEach(value->props.add(value.property().getName()+"="+value.valueName()));
    Collections.sort(props);
    return BuiltInRegistries.BLOCK.getKey(state.getBlock())+"|"+String.join(",",props);
  }
  public static void main(String[] names) throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var field=StructureTemplate.class.getDeclaredField("palettes");field.setAccessible(true);
    for(int i=0;i<names.length;i++) {
      String name=names[i];
      try(var input=ShipwreckRecordsOracle.class.getClassLoader().getResourceAsStream("data/minecraft/structure/"+name+".nbt")) {
        if(input==null) throw new AssertionError("missing "+name);
        var template=new StructureTemplate();template.load(BuiltInRegistries.BLOCK,NbtIo.readCompressed(input,NbtAccounter.unlimitedHeap()));
        var palettes=(List<StructureTemplate.Palette>)field.get(template);
        var origin=new BlockPos(-101+i*7,41+i%11,207-i*13);
        int radius=i%2==0?64:8;
        var clip=new BoundingBox(origin.getX()-radius,origin.getY()-radius,origin.getZ()-radius,origin.getX()+radius,origin.getY()+radius,origin.getZ()+radius);
        var settings=new StructurePlaceSettings().setRotation(Rotation.values()[i%4]).setRotationPivot(new BlockPos(4,0,15)).setBoundingBox(clip).addProcessor(BlockIgnoreProcessor.STRUCTURE_AND_AIR);
        var palette=settings.getRandomPalette(palettes,origin);
        for(var entry:palette.blocks()) {
          var s=entry.state();boolean full=!s.getBlock().hasDynamicShape()&&s.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO);
          out.println("I\t"+key(s)+"\t"+full);
        }
        var blocks=StructureTemplate.processBlockInfos(null,origin,BlockPos.ZERO,settings,palette.blocks());
        var markers=new ArrayList<StructureTemplate.StructureBlockInfo>();
        for(var block:template.filterBlocks(origin,settings,Blocks.STRUCTURE_BLOCK)) {
          if(block.nbt()!=null && block.nbt().read("mode",StructureMode.LEGACY_CODEC).orElseThrow()==StructureMode.DATA) markers.add(block);
        }
        out.println("T\t"+name+"\t"+blocks.size()+"\t"+markers.size());
        for(var block:blocks) {var p=block.pos();out.println("B\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()+"\t"+key(block.state())+"\t"+(block.nbt()!=null));}
        for(var marker:markers) {var p=marker.pos();out.println("M\t"+p.getX()+"\t"+p.getY()+"\t"+p.getZ()+"\t"+marker.nbt().getStringOr("metadata",""));}
      }
    }
  }
}
