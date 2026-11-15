import java.lang.reflect.Proxy;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ServerLevelAccessor;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.world.level.material.*;
import net.minecraft.world.ticks.*;

/** Compare single-chest edge updates and protochunk water ticks. */
public class ChestEdgeOracle {
    public static void main(String[] args) {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var neighbors = List.of(Blocks.AIR.defaultBlockState(), Blocks.STONE.defaultBlockState(),
            Blocks.WATER.defaultBlockState(), Blocks.TRAPPED_CHEST.defaultBlockState(),
            Blocks.CHEST.defaultBlockState());
        var pos = new BlockPos(8, 8, 8);
        for (Direction facing : Direction.Plane.HORIZONTAL) for (boolean waterlogged : List.of(false, true)) {
            for (Direction direction : Direction.values()) for (int n = 0; n < neighbors.size(); n++) {
                if (n == 4 && direction.getAxis().isHorizontal()) continue;
                var state = Blocks.CHEST.defaultBlockState().setValue(ChestBlock.FACING, facing)
                    .setValue(ChestBlock.TYPE, ChestType.SINGLE).setValue(ChestBlock.WATERLOGGED, waterlogged);
                var ticks = new ProtoChunkTicks<Fluid>();
                var requests = new ArrayList<Integer>();
                var world = (ServerLevelAccessor) Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),
                    new Class<?>[]{ServerLevelAccessor.class}, (proxy, method, values) -> switch (method.getName()) {
                        case "scheduleTick" -> {
                            if (!(values[1] instanceof Fluid fluid) || fluid != Fluids.WATER || !pos.equals(values[0]))
                                throw new AssertionError("unexpected tick");
                            int delay = (int) values[2];
                            requests.add(delay);
                            ticks.schedule(new ScheduledTick<>(fluid, pos, delay, 0L));
                            yield null;
                        }
                        default -> throw new UnsupportedOperationException(method.toString());
                    });
                var random = RandomSource.create(17);
                var updated = state;
                for (int repeat = 0; repeat < 2; repeat++)
                    updated = state.updateShape(world, world, pos, direction, pos.relative(direction), neighbors.get(n), random);
                if (updated != state || random.nextInt() != RandomSource.create(17).nextInt())
                    throw new AssertionError("unexpected state or random update");
                var saved = ticks.pack(0);
                out.printf("E %s %s %s %d %d %d %d %d%n", facing.getSerializedName(), waterlogged,
                    direction.getSerializedName(), n, requests.size(), saved.size(),
                    saved.isEmpty() ? -1 : saved.get(0).delay(),
                    saved.isEmpty() ? -1 : saved.get(0).priority().getValue());
            }
        }
    }
}
