import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HexFormat;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.BlockPos;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.DataLayer;
import net.minecraft.world.level.chunk.LightChunk;
import net.minecraft.world.level.chunk.LightChunkGetter;
import net.minecraft.world.level.lighting.LayerLightSectionStorage;
import net.minecraft.world.level.lighting.LevelLightEngine;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.level.material.FluidState;

// Exercise the 26.3 light engine using an explicit sequence of task drains.
public class QueuedSectionLightOracle implements LightChunkGetter, BlockGetter {
    static final java.io.PrintStream OUT = System.out;
    public int getMinY() { return 0; }
    public int getHeight() { return 80; }
    public BlockGetter getLevel() { return this; }
    public LightChunk getChunkForLighting(int x, int z) { return null; }
    public void onLightUpdate(LightLayer layer, SectionPos pos) {}
    public BlockEntity getBlockEntity(BlockPos pos) { return null; }
    public BlockState getBlockState(BlockPos pos) { return Blocks.AIR.defaultBlockState(); }
    public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }

    static LightLayer channel(String value) { return value.equals("B") ? LightLayer.BLOCK : LightLayer.SKY; }
    static byte[] bytes(int kind) {
        var bytes = new byte[2048];
        if (kind == 1) for (int i = 0; i < bytes.length; i++) bytes[i] = (byte)(i * 37 + 11);
        if (kind == 2) java.util.Arrays.fill(bytes, (byte)0xff);
        if (kind == 3) for (int i = 0; i < bytes.length; i++) bytes[i] = (byte)(i * 19 + 7);
        return bytes;
    }
    static void row(String source, LightLayer channel, int x, int y, int z, DataLayer layer) {
        if (layer == null) return;
        String form = layer.isDefinitelyHomogenous() ? "H" + layer.get(0, 0, 0) : "M";
        // DataLayer.getData() materializes the layer. Reading nibbles is safe.
        var raw = new byte[2048];
        for (int i = 0; i < 4096; i++) {
            int value = layer.get(i & 15, (i >> 8) & 15, (i >> 4) & 15);
            raw[i / 2] |= (byte)(value << ((i & 1) * 4));
        }
        OUT.printf("L %s %s %d %d %d %s %s%n", source,
            channel == LightLayer.BLOCK ? "B" : "S", x, y, z, form,
            HexFormat.of().formatHex(raw));
    }
    static void capture(LevelLightEngine engine, boolean block, boolean sky, String name) throws Exception {
        var storageField = LightEngine.class.getDeclaredField("storage");
        storageField.setAccessible(true);
        var visible = LayerLightSectionStorage.class.getDeclaredMethod("getDataLayer", long.class, boolean.class);
        visible.setAccessible(true);
        OUT.println("H " + name);
        for (var channel : new LightLayer[]{LightLayer.BLOCK, LightLayer.SKY}) {
            if ((channel == LightLayer.BLOCK && !block) || (channel == LightLayer.SKY && !sky)) continue;
            var listener = engine.getLayerListener(channel);
            var storage = storageField.get(listener);
            for (int x = -2; x <= 2; x++) for (int z = -2; z <= 2; z++) for (int y = -1; y <= 5; y++) {
                var pos = SectionPos.of(x, y, z);
                row("V", channel, x, y, z, (DataLayer)visible.invoke(storage, pos.asLong(), false));
                row("D", channel, x, y, z, listener.getDataLayerData(pos));
            }
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        LevelLightEngine engine = null;
        boolean block = true, sky = true;
        for (String line : Files.readAllLines(Path.of(args[0]))) {
            if (line.isBlank()) continue;
            String[] p = line.split(" ");
            switch (p[0]) {
                case "N" -> {
                    block = p[1].equals("1"); sky = p[2].equals("1");
                    engine = new LevelLightEngine(new QueuedSectionLightOracle(), block, sky);
                }
                case "Q" -> {
                    int kind = Integer.parseInt(p[5]);
                    engine.queueSectionData(channel(p[1]), SectionPos.of(Integer.parseInt(p[2]),
                        Integer.parseInt(p[3]), Integer.parseInt(p[4])), kind < 0 ? null : new DataLayer(bytes(kind)));
                }
                case "T" -> engine.updateSectionStatus(SectionPos.of(Integer.parseInt(p[1]),
                    Integer.parseInt(p[2]), Integer.parseInt(p[3])), p[4].equals("0"));
                case "E" -> engine.setLightEnabled(new ChunkPos(Integer.parseInt(p[1]), Integer.parseInt(p[2])), p[3].equals("1"));
                case "R" -> engine.retainData(new ChunkPos(Integer.parseInt(p[1]), Integer.parseInt(p[2])), p[3].equals("1"));
                case "D" -> engine.runLightUpdates();
                case "S" -> {
                    var layer = engine.getLayerListener(channel(p[1])).getDataLayerData(
                        SectionPos.of(Integer.parseInt(p[2]), Integer.parseInt(p[3]), Integer.parseInt(p[4])));
                    if (layer != null && !layer.isEmpty()) layer.getData();
                }
                case "C" -> capture(engine, block, sky, p[1]);
                default -> throw new AssertionError(line);
            }
        }
    }
}
