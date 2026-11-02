import java.nio.file.Path;
import java.util.UUID;
import net.minecraft.core.BlockPos;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.SculkSensorBlockEntity;
import net.minecraft.world.level.block.entity.CalibratedSculkSensorBlockEntity;
import net.minecraft.world.level.gameevent.GameEvent;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.phys.Vec3;

public class SculkDraftOracle {
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var registry = net.minecraft.data.registries.VanillaRegistries.createWorldLookup();
        var root = NbtIo.read(Path.of(args[0]));
        for (int n = 0; n < 8; n++) {
            var tag = (CompoundTag)root.get("sensor" + n);
            var position = new BlockPos(-1,42,3);
            boolean calibrated = (n & 4) != 0;
            var state = (calibrated ? Blocks.CALIBRATED_SCULK_SENSOR : Blocks.SCULK_SENSOR).defaultBlockState();
            var sensor = calibrated ? new CalibratedSculkSensorBlockEntity(position,state) : new SculkSensorBlockEntity(position,state);
            sensor.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING,registry,tag));
            if (sensor.getLastVibrationFrequency() != n + 1) throw new AssertionError("frequency " + n);
            var data = sensor.getVibrationData();
            var current = data.getCurrentVibration();
            if (((n & 1) != 0) != (current != null)) throw new AssertionError("current " + n);
            if (current != null && (current.gameEvent() != GameEvent.STEP || current.distance() != 4.5f || !current.pos().equals(new Vec3(2.25,42,-3.5)) || !new UUID(3,4).equals(current.uuid()) || !new UUID(5,6).equals(current.projectileOwnerUuid()))) throw new AssertionError("current value " + n);
            if (data.getTravelTimeInTicks() != ((n & 1) != 0 ? 3 : 0)) throw new AssertionError("delay " + n);
            var pending = data.getSelectionStrategy().chosenCandidate(100);
            if (((n & 2) != 0) != pending.isPresent()) throw new AssertionError("pending " + n);
            if (pending.isPresent()) {
                var event=pending.orElseThrow();
                if (event.gameEvent() != GameEvent.SPLASH || event.distance() != 1.25f || !event.pos().equals(new Vec3(-1.25,42.5,3.75)) || !new UUID(1,2).equals(event.uuid()) || event.projectileOwnerUuid()!=null) throw new AssertionError("pending value " + n);
                if (data.getSelectionStrategy().chosenCandidate(99).isPresent()) throw new AssertionError("pending tick " + n);
            }
        }
    }
}
