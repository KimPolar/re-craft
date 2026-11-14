import java.io.DataInput;
import java.io.DataInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.ListTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SpeleothemBlock;
import net.minecraft.world.level.block.state.properties.SpeleothemThickness;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.phys.Vec3;

public class StalactiteSessionDraftOracle {
    static class Falling extends FallingBlockEntity {
        Falling() { super(null, null); }
        void loadExtra(ValueInput input) { super.readAdditionalSaveData(input); }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registry = VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
        for (var pending : BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
        var unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe) unsafeField.get(null);
        var hurt = FallingBlockEntity.class.getDeclaredField("hurtEntities");
        var amount = FallingBlockEntity.class.getDeclaredField("fallDamagePerDistance");
        var maximum = FallingBlockEntity.class.getDeclaredField("fallDamageMax");
        hurt.setAccessible(true);
        amount.setAccessible(true);
        maximum.setAccessible(true);
        CompoundTag chunk;
        try (var input = new DataInputStream(Files.newInputStream(Path.of(args[0])))) {
            chunk = NbtIo.read((DataInput) input);
        }
        var entities = (ListTag) chunk.get("Entities");
        if (entities.size() != 3) throw new AssertionError("expected three falling blocks");
        var block = args[1].equals("pointed_dripstone") ? Blocks.POINTED_DRIPSTONE : Blocks.SULFUR_SPIKE;
        var thicknesses = List.of(SpeleothemThickness.MIDDLE, SpeleothemThickness.FRUSTUM, SpeleothemThickness.TIP);
        var uuids = new HashSet<List<Integer>>();
        for (int i = 0; i < entities.size(); i++) {
            var tag = (CompoundTag) entities.get(i);
            var uuid = tag.getIntArray("UUID").orElseThrow();
            if (uuid.length != 4 || !uuids.add(List.of(uuid[0], uuid[1], uuid[2], uuid[3])))
                throw new AssertionError("missing or duplicate UUID");
            var input = TagValueInput.create(ProblemReporter.DISCARDING, registry, tag);
            var entity = (Falling) unsafe.allocateInstance(Falling.class);
            entity.loadExtra(input);
            var expected = block.defaultBlockState().setValue(SpeleothemBlock.TIP_DIRECTION, Direction.DOWN)
                .setValue(SpeleothemBlock.THICKNESS, thicknesses.get(i)).setValue(SpeleothemBlock.WATERLOGGED, false);
            var pos = input.read("Pos", Vec3.CODEC).orElseThrow();
            var velocity = input.read("Motion", Vec3.CODEC).orElseThrow();
            if (entity.getBlockState() != expected || entity.time != 1 || !entity.dropItem
                || hurt.getBoolean(entity) != (i == 2) || amount.getFloat(entity) != (i == 2 ? 6.0f : 0.0f)
                || maximum.getInt(entity) != 40
                || !pos.equals(new Vec3(16_000_008.5, 10.0 - i - 0.04, -15_999_991.5))
                || !velocity.equals(new Vec3(0.0, -0.04 * (double) 0.98f, 0.0))
                || input.getDoubleOr("fall_distance", -1) != (double) 0.04f) {
                throw new AssertionError("session falling-block draft differs at " + i + ": " + tag);
            }
        }
    }
}
