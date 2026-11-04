import com.google.gson.*;
import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.server.level.WorldGenRegion;
import net.minecraft.world.entity.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.blockscan.BlockMatcher;
import net.minecraft.world.level.blockscan.BoxBlockMatcher;
import net.minecraft.world.phys.AABB;
import sun.misc.Unsafe;

public final class CreatureObstructionOracle {
    static final class Region extends WorldGenRegion {
        Map<Long, ProtoChunk> chunks;
        PalettedContainerFactory palette;
        LevelHeightAccessor height;
        Region() { super(null, null, null, null); }
        @Override public int getMinY() { return -16; }
        @Override public int getHeight() { return 32; }
        // Preserve the real matcher while bypassing only WorldGenRegion's
        // write-zone warning, whose scheduling fields this fixture lacks.
        @Override public BlockMatcher findBlocksIn(BlockPos from, BlockPos to) { return new BoxBlockMatcher(this,from,to); }
        @Override public ChunkAccess getChunk(int x, int z) {
            return chunks.computeIfAbsent(ChunkPos.pack(x,z), ignored -> {
                var chunk = new ProtoChunk(new ChunkPos(x,z), UpgradeData.EMPTY, height, palette, null);
                chunk.setPersistedStatus(ChunkStatus.TERRAIN); return chunk;
            });
        }
    }
    @SuppressWarnings({"unchecked", "rawtypes"})
    static BlockState property(BlockState state, String key, String value) {
        Property prop = state.getBlock().getStateDefinition().getProperty(key);
        return state.setValue(prop, (Comparable) prop.getValue(value).orElseThrow());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe = (Unsafe)field.get(null);
        var biome = Holder.direct((Biome)unsafe.allocateInstance(Biome.class));
        var ids = new IdMapper<Holder<Biome>>(); ids.add(biome);
        var palette = new PalettedContainerFactory(Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY), Blocks.AIR.defaultBlockState(), null, Strategy.createForBiomes(ids), biome, null);
        var region = (Region)unsafe.allocateInstance(Region.class); region.palette = palette; region.height = LevelHeightAccessor.create(-16,32);
        var entities = new HashMap<String,Class<?>>();
        for (String name : List.of("pig","cow","sheep","chicken","horse","goat","rabbit")) {
            var type = (ParameterizedType)EntityTypes.class.getField(name.toUpperCase(Locale.ROOT)).getGenericType();
            var clazz = (Class<?>)type.getActualTypeArguments()[0];
            if (clazz.getMethod("checkSpawnObstruction", LevelReader.class).getDeclaringClass() != Mob.class)
                throw new AssertionError("unsupported animal obstruction " + name);
            entities.put(name,clazz);
        }
        var input = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray(); var output = new JsonArray();
        for (var item : input) {
            var test = item.getAsJsonObject(); region.chunks = new HashMap<>();
            for (var element : test.getAsJsonArray("blocks")) {
                var block = element.getAsJsonObject(); var p = block.getAsJsonArray("position");
                int x=p.get(0).getAsInt(), y=p.get(1).getAsInt(), z=p.get(2).getAsInt();
                if (y < -16 || y >= 16) continue;
                var state = BuiltInRegistries.BLOCK.getValue(Identifier.parse(block.get("name").getAsString())).defaultBlockState();
                for (var e : block.getAsJsonObject("properties").entrySet()) state = property(state,e.getKey(),e.getValue().getAsString());
                var chunk = (ProtoChunk)region.getChunk(x >> 4,z >> 4);
                chunk.getSection((y + 16) >> 4).setBlockState(x & 15,y & 15,z & 15,state,false);
            }
            var min=test.getAsJsonArray("min"); var max=test.getAsJsonArray("max");
            var box=new AABB(min.get(0).getAsDouble(),min.get(1).getAsDouble(),min.get(2).getAsDouble(),max.get(0).getAsDouble(),max.get(1).getAsDouble(),max.get(2).getAsDouble());
            var mob=(Mob)unsafe.allocateInstance(entities.get(test.get("entity").getAsString())); mob.setBoundingBox(box);
            boolean liquid = region.containsAnyLiquid(box);
            if (!region.isUnobstructed(mob)) throw new AssertionError("worldgen entity query was not empty");
            boolean actual = mob.checkSpawnObstruction(region);
            if (actual == liquid) throw new AssertionError("unexpected obstruction behavior");
            output.add(actual);
        }
        Files.writeString(Path.of(args[1]),output.toString());
    }
}
