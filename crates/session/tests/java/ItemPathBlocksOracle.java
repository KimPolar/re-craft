import java.lang.reflect.Method;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.core.BlockPos;

// Check the actual 26.3 block implementations used by the Rust path proof.
public class ItemPathBlocksOracle {
    static Class<?> owner(Class<?> type, String method) {
        for (Class<?> current = type; current != null; current = current.getSuperclass()) {
            for (Method candidate : current.getDeclaredMethods()) {
                if (candidate.getName().equals(method)) return current;
            }
        }
        throw new AssertionError("missing block method " + method);
    }

    public static void main(String[] names) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (String name : names) {
            var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse(name));
            if (block == null || !BuiltInRegistries.BLOCK.getKey(block).toString().equals(name))
                throw new AssertionError("unknown block: " + name);
            for (String method : new String[] {"stepOn", "fallOn", "bounceOn"}) {
                if (owner(block.getClass(), method) != Block.class)
                    throw new AssertionError(name + " overrides " + method);
            }
            for (String method : new String[] {"entityInside", "getEntityInsideCollisionShape"}) {
                if (owner(block.getClass(), method) != BlockBehaviour.class)
                    throw new AssertionError(name + " overrides " + method);
            }
            if (block.getFriction() != 0.6f || block.getSpeedFactor() != 1.0f || block.getJumpFactor() != 1.0f)
                throw new AssertionError(name + " has unusual movement factors");
            var states = block.getStateDefinition().getPossibleStates();
            int expected = name.equals("minecraft:grass_block") ? 2 : 1;
            if (states.size() != expected || states.stream().anyMatch(state -> !state.getFluidState().isEmpty()))
                throw new AssertionError(name + " has unexpected states or fluids");
            if (!name.equals("minecraft:grass_block") && states.stream().anyMatch(state -> !state.getProperties().isEmpty()))
                throw new AssertionError(name + " has unexpected properties");
            boolean expectedAir = name.equals("minecraft:air") || name.equals("minecraft:cave_air") || name.equals("minecraft:void_air");
            for (var state : states) {
                var shape = state.getCollisionShape(EmptyBlockGetter.INSTANCE, BlockPos.ZERO);
                if (expectedAir ? !shape.isEmpty() : !Block.isShapeFullBlock(shape))
                    throw new AssertionError(name + " has unexpected collision shape");
            }
            if (name.equals("minecraft:grass_block") && states.stream().anyMatch(state ->
                state.getProperties().size() != 1 || state.getProperties().stream().noneMatch(property -> property.getName().equals("snowy"))))
                throw new AssertionError("unexpected grass properties");
            if (name.equals("minecraft:grass_block") && !states.stream().flatMap(state -> state.getValues().map(value -> value.valueName())).collect(java.util.stream.Collectors.toSet()).equals(java.util.Set.of("true", "false")))
                throw new AssertionError("unexpected grass property values");
        }
        var soulSand = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:soul_sand"));
        var portal = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:nether_portal"));
        var redstone = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:redstone_ore"));
        if (soulSand.getSpeedFactor() == 1.0f || owner(portal.getClass(), "entityInside") == BlockBehaviour.class
            || owner(redstone.getClass(), "stepOn") == Block.class)
            throw new AssertionError("hazard controls did not detect special behavior");
    }
}
