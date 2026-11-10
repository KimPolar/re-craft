import java.nio.file.Path;
import java.util.List;
import java.util.UUID;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.decoration.Cushion;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.TagValueOutput;
import net.minecraft.world.phys.Vec3;

public class GeneratedCushionTemplateOracle {
    static class World extends ServerLevel {
        World() { super(null, null, null, null, null, null, false, 0L, List.of(), false); }
        @Override public int getNextEntityId() { return 1; }
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var world = (World) ((sun.misc.Unsafe) field.get(null)).allocateInstance(World.class);
        var lookup = VanillaRegistries.createWorldLookup();
        String[] colors = {"white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black"};
        float[] yaws = {-450, 0, 90, 180, 270};
        try (var jar = new ZipFile(args[0]);
             var input = jar.getInputStream(jar.getEntry("data/minecraft/structure/abandoned_camp/camp/bamboo_jungle/campsite_bamboo_jungle_1.nbt"))) {
            var root = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap());
            if (root.getIntOr("DataVersion", -1) != 5023) throw new AssertionError("template version differs");
            var source = (CompoundTag) root.getListOrEmpty("entities").get(0);
            var local = source.read("pos", Vec3.CODEC).orElseThrow();
            var anchor = source.read("blockPos", BlockPos.CODEC).orElseThrow();
            var origin = new BlockPos(-32, 80, 48);
            int index = 0;
            for (String color : colors) for (float inputYaw : yaws) for (var rotation : Rotation.values()) {
                var point = StructureTemplate.transform(local, Mirror.NONE, rotation, BlockPos.ZERO).add(Vec3.atLowerCornerOf(origin));
                var block = StructureTemplate.transform(anchor, Mirror.NONE, rotation, BlockPos.ZERO).offset(origin);
                var nbt = source.getCompoundOrEmpty("nbt").copy();
                nbt.putString("color", color);
                var angles = new ListTag();
                angles.add(FloatTag.valueOf(inputYaw)); angles.add(FloatTag.valueOf(0));
                nbt.put("Rotation", angles);
                nbt.store("Pos", Vec3.CODEC, point);
                nbt.remove("UUID");
                if (nbt.contains("block_pos")) nbt.store("block_pos", BlockPos.CODEC, block);
                var cushion = new Cushion(EntityTypes.CUSHION, world);
                cushion.load(TagValueInput.create(ProblemReporter.DISCARDING, lookup, nbt));
                cushion.setUUID(new UUID(1, 2));
                float yaw = cushion.rotate(rotation) + (cushion.mirror(Mirror.NONE) - cushion.getYRot());
                cushion.snapTo(point.x, point.y, point.z, yaw, cushion.getXRot());
                var output = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, lookup);
                if (!cushion.save(output)) throw new AssertionError("cushion was not saved");
                NbtIo.write(output.buildResult(), Path.of(args[1]).resolve("cushion-" + index++ + ".nbt"));
            }
            out.println("CASES " + index);
        }
        int entities = 0;
        try (var jar = new ZipFile(args[0])) {
            var entries = jar.entries();
            while (entries.hasMoreElements()) {
                var entry = entries.nextElement();
                if (!entry.getName().startsWith("data/minecraft/structure/") || !entry.getName().endsWith(".nbt")) continue;
                try (var input = jar.getInputStream(entry)) {
                    var root = NbtIo.readCompressed(input, NbtAccounter.unlimitedHeap());
                    for (var value : root.getListOrEmpty("entities")) {
                        if (!(value instanceof CompoundTag source)) throw new AssertionError("invalid entity");
                        var sourceNbt = source.getCompoundOrEmpty("nbt");
                        if (!sourceNbt.getStringOr("id", "").equals("minecraft:cushion")) continue;
                        if (root.getIntOr("DataVersion", -1) != 5023) throw new AssertionError("template version differs");
                        NbtIo.write(source, Path.of(args[1]).resolve("vanilla-source-" + entities + ".nbt"));
                        var local = source.read("pos", Vec3.CODEC).orElseThrow();
                        var anchor = source.read("blockPos", BlockPos.CODEC).orElseThrow();
                        var origin = new BlockPos(-32, 80, 48);
                        for (var rotation : Rotation.values()) {
                            var point = StructureTemplate.transform(local, Mirror.NONE, rotation, BlockPos.ZERO).add(Vec3.atLowerCornerOf(origin));
                            var block = StructureTemplate.transform(anchor, Mirror.NONE, rotation, BlockPos.ZERO).offset(origin);
                            var nbt = sourceNbt.copy();
                            nbt.store("Pos", Vec3.CODEC, point);
                            nbt.remove("UUID");
                            if (nbt.contains("block_pos")) nbt.store("block_pos", BlockPos.CODEC, block);
                            var cushion = new Cushion(EntityTypes.CUSHION, world);
                            cushion.load(TagValueInput.create(ProblemReporter.DISCARDING, lookup, nbt));
                            cushion.setUUID(new UUID(1, 2));
                            float yaw = cushion.rotate(rotation) + (cushion.mirror(Mirror.NONE) - cushion.getYRot());
                            cushion.snapTo(point.x, point.y, point.z, yaw, cushion.getXRot());
                            var output = TagValueOutput.createWithContext(ProblemReporter.DISCARDING, lookup);
                            if (!cushion.save(output)) throw new AssertionError("cushion was not saved");
                            NbtIo.write(output.buildResult(), Path.of(args[1]).resolve("vanilla-" + entities + "-" + rotation.ordinal() + ".nbt"));
                        }
                        entities++;
                    }
                }
            }
        }
        out.println("VANILLA " + entities);
    }
}
