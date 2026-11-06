import java.util.ArrayList;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.FrontAndTop;
import net.minecraft.resources.Identifier;
import net.minecraft.util.Util;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.JigsawBlock;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.entity.JigsawBlockEntity.JointType;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.structure.pools.SinglePoolElement;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate.JigsawBlockInfo;

public class JigsawConnectorOracle {
    static JigsawBlockInfo connector(FrontAndTop orientation, JointType joint, Identifier name, Identifier target, int index, int priority) {
        var state = Blocks.JIGSAW.defaultBlockState().setValue(JigsawBlock.ORIENTATION, orientation);
        return new JigsawBlockInfo(new BlockPos(index, 0, 0), state, joint, name, null, target, 0, priority);
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var id = Identifier.parse("example:door");
        var other = Identifier.parse("example:other");
        var orientations = FrontAndTop.values();
        for (int a = 0; a < orientations.length; a++) {
            var state = Blocks.JIGSAW.defaultBlockState().setValue(JigsawBlock.ORIENTATION, orientations[a]);
            out.printf("D\t%d\t%s%n", a, StructureTemplate.getDefaultJointType(state).getSerializedName());
            for (int r = 0; r < Rotation.values().length; r++) {
                var rotated = state.rotate(Rotation.values()[r]).getValue(JigsawBlock.ORIENTATION);
                out.printf("R\t%d\t%d\t%s%n", a, r, rotated.getSerializedName());
            }
            for (int b = 0; b < orientations.length; b++) for (int j = 0; j < 2; j++) for (int n = 0; n < 3; n++) {
                var parent = connector(orientations[a], j == 0 ? JointType.ALIGNED : JointType.ROLLABLE, id, id, 0, 0);
                var child = connector(orientations[b], JointType.ALIGNED, n == 0 ? id : n == 1 ? other : null, other, 0, 0);
                out.printf("A\t%d\t%d\t%d\t%d\t%s%n", a, b, j, n, JigsawBlock.canAttach(parent, child));
            }
        }
        var sort = SinglePoolElement.class.getDeclaredMethod("sortBySelectionPriority", List.class);
        sort.setAccessible(true);
        for (long seed : new long[]{0, 1, -1, 42, Long.MIN_VALUE, Long.MAX_VALUE}) {
            int[] priorities = {2, -1, 0, 2, 0, Integer.MAX_VALUE, Integer.MIN_VALUE, 2};
            var list = new ArrayList<JigsawBlockInfo>();
            for (int i = 0; i < priorities.length; i++) list.add(connector(orientations[0], JointType.ALIGNED, id, id, i, priorities[i]));
            var random = new LegacyRandomSource(seed);
            Util.shuffle(list, random);
            sort.invoke(null, list);
            var order = new StringBuilder();
            for (var item : list) order.append(item.pos().getX());
            out.printf("S\t%d\t%s\t%d%n", seed, order, random.nextLong());
        }
    }
}
