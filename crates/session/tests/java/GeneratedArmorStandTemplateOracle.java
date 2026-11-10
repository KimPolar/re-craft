import java.nio.file.Path;
import java.util.List;
import java.util.UUID;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.component.DataComponentMap;
import net.minecraft.core.component.DataComponents;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.*;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.item.Items;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.TagValueOutput;
import net.minecraft.world.phys.Vec3;

/** Exercise actual 26.3 template entities; no game resources are copied to the repository. */
public class GeneratedArmorStandTemplateOracle {
    static class World extends ServerLevel {
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getNextEntityId() { return 1; }
        @Override public FeatureFlagSet enabledFeatures() { return FeatureFlags.VANILLA_SET; }
    }
    public static void main(String[] args) throws Exception {
        var stdout = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        // The data-driven item registry is not initialized by Bootstrap alone.
        // These two unmodified stacks need their standard maximum stack size to decode.
        var defaults = DataComponentMap.builder().set(DataComponents.MAX_STACK_SIZE, 1).build();
        Items.IRON_HELMET.builtInRegistryHolder().bindComponents(defaults);
        Items.IRON_CHESTPLATE.builtInRegistryHolder().bindComponents(defaults);
        var f = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        f.setAccessible(true);
        var world = (World) ((sun.misc.Unsafe) f.get(null)).allocateInstance(World.class);
        var lookup = VanillaRegistries.createWorldLookup();
        int count = 0;
        try (var jar = new ZipFile(args[0]);
             var input = jar.getInputStream(jar.getEntry("data/minecraft/structure/village/taiga/houses/taiga_armorer_2.nbt"))) {
            var root = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap());
            if (root.getIntOr("DataVersion", -1) != 5023) throw new AssertionError("template version differs");
            for (var value : root.getListOrEmpty("entities")) {
                var source = (CompoundTag) value;
                if (!source.getCompoundOrEmpty("nbt").getStringOr("id", "").equals("minecraft:armor_stand")) throw new AssertionError("unexpected entity");
                NbtIo.write(source, Path.of(args[1]).resolve("source-" + count + ".nbt"));
                var local = source.read("pos", Vec3.CODEC).orElseThrow();
                var origin = new BlockPos(-32, 80, 48);
                for (var rotation : Rotation.values()) for (int pivotIndex = 0; pivotIndex < 2; pivotIndex++) {
                    var pivot = pivotIndex == 0 ? BlockPos.ZERO : new BlockPos(4, 0, 15);
                    var point = StructureTemplate.transform(local, Mirror.NONE, rotation, pivot).add(Vec3.atLowerCornerOf(origin));
                    var nbt = source.getCompoundOrEmpty("nbt").copy();
                    nbt.store("Pos", Vec3.CODEC, point);
                    nbt.remove("UUID");
                    var problems = new ProblemReporter.Collector();
                    var entity = EntityType.create(TagValueInput.create(problems, lookup, nbt), world,
                        new EntitySpawnRequest(EntitySpawnReason.STRUCTURE, false)).orElseThrow();
                    if (!problems.isEmpty()) throw new AssertionError(problems.getReport());
                    entity.setUUID(new UUID(1, 2));
                    float yaw = entity.rotate(rotation) + (entity.mirror(Mirror.NONE) - entity.getYRot());
                    entity.snapTo(point.x, point.y, point.z, yaw, entity.getXRot());
                    entity.setYBodyRot(yaw);
                    entity.setYHeadRot(yaw);
                    var output = TagValueOutput.createWithContext(problems, lookup);
                    if (!entity.save(output) || !problems.isEmpty()) throw new AssertionError(problems.getReport());
                    NbtIo.write(output.buildResult(), Path.of(args[1]).resolve("saved-" + count + "-" + rotation.ordinal() + "-" + pivotIndex + ".nbt"));
                }
                count++;
            }
        }
        stdout.println("STANDS " + count);
    }
}
