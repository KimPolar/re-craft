import it.unimi.dsi.fastutil.HashCommon;
import it.unimi.dsi.fastutil.longs.Long2LongMaps;
import it.unimi.dsi.fastutil.longs.Long2LongOpenHashMap;
import net.minecraft.world.level.ChunkPos;

public class LevelTickIndexOracle {
    static long key(int i) {
        return ChunkPos.pack((i * 37) % 97 - 48, (i * 29) % 89 - 44);
    }
    static void drain(Long2LongOpenHashMap map, int round, long time) {
        var it = Long2LongMaps.fastIterator(map);
        StringBuilder line = new StringBuilder("round " + round);
        while (it.hasNext()) {
            var e = it.next();
            long k = e.getLongKey();
            int x = ChunkPos.getX(k), z = ChunkPos.getZ(k);
            if (e.getLongValue() <= time && Math.floorMod(x * 17 + z * 31 + round, 4) != 0) {
                line.append(' ').append(x).append(':').append(z);
                it.remove();
            }
        }
        System.out.println(line);
    }
    public static void main(String[] args) {
        var map = new Long2LongOpenHashMap();
        map.put(ChunkPos.pack(0, 0), -100);
        map.put(ChunkPos.pack(Integer.MIN_VALUE, Integer.MAX_VALUE), -100);
        for (int i = 0; i < 100; i++) map.put(key(i), i % 17 - 8);
        for (int i = 0; i < 30; i += 3) map.remove(key(i));
        for (int round = 0; round < 8; round++) {
            drain(map, round, round - 2);
            for (int i = 0; i < 31; i++) map.put(key((round * 19 + i * 7) % 150), (i + round) % 13 - 6);
            for (int i = 0; i < 11; i += 2) map.remove(key((round * 13 + i) % 150));
        }
        drain(map, 8, Long.MAX_VALUE);
        var wrapped = new Long2LongOpenHashMap();
        int count = 0;
        for (int x = 1; count < 24; x++) {
            long key = ChunkPos.pack(x, 0);
            if ((HashCommon.mix(key) & 31) == 31) {
                wrapped.put(key, 0);
                count++;
            }
        }
        for (int round = 20; round < 24; round++) drain(wrapped, round, 0);
    }
}
