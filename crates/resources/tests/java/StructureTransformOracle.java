import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Vec3i;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

public class StructureTransformOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var bounds = StructureTemplate.class.getDeclaredMethod("getBoundingBox", BlockPos.class, Rotation.class, BlockPos.class, Mirror.class, Vec3i.class);
        bounds.setAccessible(true);
        for (var rotation : Rotation.values()) for (var mirror : Mirror.values()) for (int i=0;i<128;i++) {
            var local = new BlockPos(i*7-500, i-70, 300-i*11);
            var pivot = new BlockPos(i%11-5, 19, i%13-6);
            var origin = new BlockPos(i*16-900, -64+i%9, 700-i*8);
            var size = new Vec3i(i%17, i%19, i%23);
            var block = StructureTemplate.transform(local,mirror,rotation,pivot);
            var entity = StructureTemplate.transform(new Vec3(i*0.25-17.125, i*0.125-4.5, 21.375-i*0.5),mirror,rotation,pivot);
            var box = (BoundingBox) bounds.invoke(null,origin,rotation,pivot,mirror,size);
            System.out.printf("T %d %d %d %d %d %d %d %d %d %d %d %d %d %d %d%n",rotation.ordinal(),mirror.ordinal(),i,
                block.getX(),block.getY(),block.getZ(), Double.doubleToRawLongBits(entity.x),Double.doubleToRawLongBits(entity.y),Double.doubleToRawLongBits(entity.z),
                box.minX(),box.minY(),box.minZ(),box.maxX(),box.maxY(),box.maxZ());
        }
    }
}
