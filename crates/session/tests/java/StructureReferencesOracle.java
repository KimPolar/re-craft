import com.google.gson.JsonParser;
import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.*;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biomes;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pieces.*;

public class StructureReferencesOracle {
    static class Piece extends StructurePiece {
        Piece(BoundingBox box) { super(null, 0, box); }
        protected void addAdditionalSaveData(StructurePieceSerializationContext context, CompoundTag tag) {}
        public void postProcess(WorldGenLevel level, StructureManager manager, ChunkGenerator generator,
                RandomSource random, BoundingBox box, ChunkPos pos, BlockPos blockPos) {}
    }

    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registries = VanillaRegistries.createWorldLookup();
        var biome = registries.lookupOrThrow(Registries.BIOME).getOrThrow(Biomes.PLAINS);
        var ids = new IdMapper<Holder<net.minecraft.world.level.biome.Biome>>();
        ids.add(biome);
        var palette = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY),
                Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(ids), biome, null);
        var generator = WorldPresets.getNormalOverworld(registries).generator();
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        for (var element : input) {
            var test = element.getAsJsonObject();
            var target = new ChunkPos(test.get("x").getAsInt(), test.get("z").getAsInt());
            Map<Long, ProtoChunk> chunks = new HashMap<>();
            Map<Structure, Identifier> structureIds = new IdentityHashMap<>();
            java.util.function.BiFunction<Integer,Integer,ProtoChunk> chunk = (x,z) -> chunks.computeIfAbsent(
                    ChunkPos.pack(x,z), key -> new ProtoChunk(new ChunkPos(x,z), UpgradeData.EMPTY,
                            LevelHeightAccessor.create(0,16), palette, null));
            for (var item : test.getAsJsonArray("starts")) {
                var s = item.getAsJsonObject();
                var id = Identifier.parse(s.get("id").getAsString());
                var structure = registries.lookupOrThrow(Registries.STRUCTURE)
                        .getOrThrow(ResourceKey.create(Registries.STRUCTURE,id)).value();
                structureIds.put(structure, id);
                int x=s.get("x").getAsInt(), z=s.get("z").getAsInt();
                List<StructurePiece> pieces = new ArrayList<>();
                for (var pieceElement : s.getAsJsonArray("pieces")) {
                    var b = pieceElement.getAsJsonArray();
                    var box = new BoundingBox(b.get(0).getAsInt(),b.get(1).getAsInt(),b.get(2).getAsInt(),
                            b.get(3).getAsInt(),b.get(4).getAsInt(),b.get(5).getAsInt());
                    pieces.add(new Piece(box));
                }
                var start = new StructureStart(structure,new ChunkPos(x,z),0,new PiecesContainer(pieces));
                var bounds = start.getBoundingBox();
                out.printf("BOUND %d %d %s %d %d %d %d %d %d%n", x,z,id,
                        bounds.minX(),bounds.minY(),bounds.minZ(),bounds.maxX(),bounds.maxY(),bounds.maxZ());
                chunk.apply(x,z).setStartForStructure(structure,start);
            }
            var world = (WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),
                    new Class<?>[]{WorldGenLevel.class}, (p,m,a) -> {
                        if (m.getName().equals("getChunk") && a.length == 2)
                            return chunk.apply((Integer)a[0],(Integer)a[1]);
                        throw new AssertionError(m.toString());
                    });
            var result=chunk.apply(target.x(),target.z());
            generator.createReferences(world,new StructureManager(null,null,null),result);
            for (var entry : result.getAllReferences().entrySet()) {
                var id=structureIds.get(entry.getKey());
                for (long packed : entry.getValue())
                    out.println("REF " + id + " " + packed);
            }
            out.println("END");
        }
    }
}
