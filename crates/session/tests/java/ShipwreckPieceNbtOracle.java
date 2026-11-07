import java.lang.reflect.Field;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.pieces.StructurePieceSerializationContext;
import net.minecraft.world.level.levelgen.structure.structures.ShipwreckPieces.ShipwreckPiece;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ShipwreckPieceNbtOracle {
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    Field f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true);
    var manager=(StructureTemplateManager)((sun.misc.Unsafe)f.get(null)).allocateInstance(StructureTemplateManager.class);
    var repository=StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
    var template=new StructureTemplate(); var size=StructureTemplate.class.getDeclaredField("size");size.setAccessible(true);
    var id=Identifier.withDefaultNamespace("shipwreck/with_mast");
    repository.set(manager,new HashMap<>(Map.of(id,Optional.of(template))));
    var context=new StructurePieceSerializationContext(null,null,manager);
    for (int i=0;i<32;i++) {
      boolean large=(i&1)!=0, beached=(i&2)!=0, adjusted=(i&4)!=0, stale=(i&8)!=0;
      size.set(template,large?new Vec3i(33,8,13):new Vec3i(7,4,5));
      int y=adjusted&&(!stale||!large)?40:90;
      var piece=new ShipwreckPiece(manager,id,new BlockPos(-51,y,79),Rotation.values()[i%4],beached);
      if(adjusted) piece.adjustPositionHeight(40);
      var tag=piece.createTag(context);
      if(args.length>1 && args[1].equals("verify")) {
        var rust=NbtIo.read(Path.of(args[0],"rust-"+i+".nbt"));
        if(!tag.equals(rust)) throw new AssertionError("Rust piece mismatch " + i);
        var restored=new ShipwreckPiece(manager,rust);
        var expected=new ShipwreckPiece(manager,tag).createTag(context);
        if(!expected.equals(restored.createTag(context))) throw new AssertionError("Rust reload mismatch " + i);
        continue;
      }
      NbtIo.write(tag,Path.of(args[0],"piece-"+i+".nbt"));
      var restored=new ShipwreckPiece(manager,tag);
      NbtIo.write(restored.createTag(context),Path.of(args[0],"reloaded-"+i+".nbt"));
    }
  }
}
