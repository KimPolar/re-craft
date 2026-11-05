import java.util.*;

public class StructureHashOrderOracle {
    static final class Key {
        final int id, hash;
        Key(int id, int hash) { this.id = id; this.hash = hash; }
        public int hashCode() { return hash; }
    }
    static int hash(int scenario, int id) {
        if (scenario == 1) return id * 64;
        if (scenario == 2) return id * 16;
        return id * 0x9e3779b9;
    }
    static void output(int mode, int scenario, int step, int duplicate, HashMap<Key, Integer> map) {
        System.out.print("M " + mode + " " + scenario + " " + step + " " + duplicate);
        for (Key key : map.keySet()) System.out.print(" " + key.id);
        System.out.println();
    }
    public static void main(String[] args) {
        for (int mode = 0; mode < 2; mode++) {
            for (int scenario = 0; scenario < 3; scenario++) {
                int count = scenario == 0 ? 80 : scenario == 1 ? 8 : 15;
                var map = new HashMap<Key, Integer>();
                for (int i = 0; i < count; i++) {
                    var key = new Key(i, hash(scenario, i));
                    if (mode == 0) map.put(key, i); else map.computeIfAbsent(key, k -> k.id);
                    output(mode, scenario, i, 0, map);
                    if (i % 3 == 0) {
                        if (mode == 0) map.put(key, i); else map.computeIfAbsent(key, k -> k.id);
                        output(mode, scenario, i, 1, map);
                    }
                }
            }
        }
    }
}
