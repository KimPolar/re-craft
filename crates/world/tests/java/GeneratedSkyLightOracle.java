import java.nio.file.*;
import java.util.*;
import java.util.function.BiConsumer;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.chunk.*;
import net.minecraft.world.level.lighting.*;
import net.minecraft.world.level.material.FluidState;

public class GeneratedSkyLightOracle implements LightChunkGetter, BlockGetter {
    private final Map<BlockPos, BlockState> blocks = new HashMap<>();
    private final Map<ChunkPos, TestChunk> chunks = new HashMap<>();
    public int getMinY() { return 0; }
    public int getHeight() { return 32; }
    public BlockGetter getLevel() { return this; }
    public LightChunk getChunkForLighting(int x, int z) { return chunks.get(new ChunkPos(x,z)); }
    public BlockEntity getBlockEntity(BlockPos p) { return null; }
    public BlockState getBlockState(BlockPos p) { return blocks.getOrDefault(p, Blocks.AIR.defaultBlockState()); }
    public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
    class TestChunk implements LightChunk {
        final int x,z;
        final ChunkSkyLightSources sources = new ChunkSkyLightSources(GeneratedSkyLightOracle.this);
        TestChunk(int x, int z) { this.x=x; this.z=z; }
        public BlockState getBlockState(BlockPos p) { return GeneratedSkyLightOracle.this.getBlockState(new BlockPos(x*16+Math.floorMod(p.getX(),16),p.getY(),z*16+Math.floorMod(p.getZ(),16))); }
        public FluidState getFluidState(BlockPos p) { return getBlockState(p).getFluidState(); }
        public BlockEntity getBlockEntity(BlockPos p) { return null; }
        public int getMinY() { return 0; }
        public int getHeight() { return 32; }
        public ChunkSkyLightSources getSkyLightSources() { return sources; }
        public void findBlockLightSources(BiConsumer<BlockPos,BlockState> f) {}
    }
    static class FakeChunk extends ProtoChunk {
        LevelChunkSection[] sections;
        FakeChunk() { super(null,null,null,null,null); }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public LevelChunkSection getSection(int i) { return sections[i]; }
        @Override public int getHighestFilledSectionIndex() {
            for (int i=sections.length-1;i>=0;i--) if (!sections[i].hasOnlyAir()) return i;
            return -1;
        }
    }
    static BlockState state(String name) {
        return switch(name) {
            case "stone" -> Blocks.STONE.defaultBlockState();
            case "water" -> Blocks.WATER.defaultBlockState();
            case "glass" -> Blocks.GLASS.defaultBlockState();
            case "slab_bottom" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.BOTTOM);
            case "slab_top" -> Blocks.STONE_SLAB.defaultBlockState().setValue(SlabBlock.TYPE,SlabType.TOP);
            default -> throw new IllegalArgumentException(name);
        };
    }
    static JsonArray capture(LevelLightEngine engine) {
        var result=new JsonArray(); var listener=engine.getLayerListener(LightLayer.SKY);
        for (int y=-16;y<48;y++) for (int z=0;z<16;z++) for (int x=0;x<16;x++) result.add(listener.getLightValue(new BlockPos(x,y,z)));
        return result;
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var world = new GeneratedSkyLightOracle();
        for (var entry:JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray()) {
            var o=entry.getAsJsonObject();
            world.blocks.put(new BlockPos(o.get("x").getAsInt(),o.get("y").getAsInt(),o.get("z").getAsInt()),state(o.get("state").getAsString()));
        }
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var engine=new LevelLightEngine(world,false,true);
        for (int z=-1;z<=2;z++) for (int x=-1;x<=2;x++) {
            var chunk=world.new TestChunk(x,z); world.chunks.put(new ChunkPos(x,z),chunk);
            var fake=(FakeChunk)unsafe.allocateInstance(FakeChunk.class);
            fake.sections=new LevelChunkSection[2];
            for (int sy=0;sy<2;sy++) {
                var container=new PalettedContainer<>(Blocks.AIR.defaultBlockState(),Strategy.createForBlockStates(Block.BLOCK_STATE_REGISTRY));
                fake.sections[sy]=new LevelChunkSection(container,null);
                engine.updateSectionStatus(SectionPos.of(x,sy,z),false);
            }
            for (var e:world.blocks.entrySet()) {
                var p=e.getKey();
                if (Math.floorDiv(p.getX(),16)==x && Math.floorDiv(p.getZ(),16)==z)
                    fake.sections[p.getY()/16].setBlockState(Math.floorMod(p.getX(),16),p.getY()%16,Math.floorMod(p.getZ(),16),e.getValue());
            }
            chunk.sources.fillFrom(fake);
        }
        while (engine.hasLightWork()) engine.runLightUpdates();
        if (args.length>2 && args[2].equals("staged")) {
            var results=new JsonArray(); results.add(capture(engine));
            var order=(args.length>3 ? args[3] : "0,1,-1").split(",");
            for (var value:order) {
                int x=Integer.parseInt(value);
                engine.propagateLightSources(new ChunkPos(x,0));
                while (engine.hasLightWork()) engine.runLightUpdates();
                results.add(capture(engine));
            }
            Files.writeString(Path.of(args[1]),results.toString());
            return;
        }
        for (int z=-1;z<=2;z++) for (int x=-1;x<=2;x++) engine.propagateLightSources(new ChunkPos(x,z));
        while (engine.hasLightWork()) engine.runLightUpdates();
        Files.writeString(Path.of(args[1]),capture(engine).toString());
    }
}
