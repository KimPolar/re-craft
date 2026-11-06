import net.minecraft.world.Difficulty;
import net.minecraft.world.DifficultyInstance;

public class LocalDifficultyOracle {
    public static void main(String[] args) {
        long[] times = {Long.MIN_VALUE, -1, 0, 71999, 72000, 72001, 1512000, Long.MAX_VALUE};
        long[] inhabited = {Long.MIN_VALUE, -1, 0, 1, 1800000, 3600000, Long.MAX_VALUE};
        float[] moon = {0f, .25f, .5f, 1f, -0f, Float.NaN, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY};
        for (var difficulty : Difficulty.values()) for (long time : times) for (long age : inhabited) for (float light : moon) {
            var value = new DifficultyInstance(difficulty, time, age, light);
            System.out.println("D\t" + difficulty.getId() + "\t" + time + "\t" + age + "\t" + Float.floatToIntBits(light) + "\t" + Float.floatToIntBits(value.getEffectiveDifficulty()) + "\t" + Float.floatToIntBits(value.getSpecialMultiplier()));
        }
    }
}
