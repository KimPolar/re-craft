import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.NbtIo;
import net.minecraft.resources.Identifier;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.effect.MobEffectInstance;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.Player;
import net.minecraft.world.level.storage.TagValueInput;

public class MovementEffectSaveOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var lookup = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        var player = (ServerPlayer) ((sun.misc.Unsafe) field.get(null)).allocateInstance(ServerPlayer.class);
        JsonArray output = new JsonArray();
        for (int index = 0; index < Integer.parseInt(args[1]); index++) {
            var document = NbtIo.read(Path.of(args[0], index + ".nbt"));
            var input = TagValueInput.create(ProblemReporter.DISCARDING, lookup, document);
            var attributes = new AttributeMap(Player.createAttributes().build());
            input.read("attributes", AttributeInstance.Packed.LIST_CODEC).ifPresent(attributes::apply);
            var effects = new ArrayList<>(input.read("active_effects", MobEffectInstance.CODEC.listOf()).orElse(List.of()));
            var rows = new JsonArray();
            for (int tick = 0; tick < 6; tick++) {
                var iterator = effects.iterator();
                while (iterator.hasNext()) {
                    var effect = iterator.next();
                    var type = effect.getEffect().value();
                    if (!effect.tickServer(null, player, () -> {
                        type.removeAttributeModifiers(attributes);
                        type.addAttributeModifiers(attributes, effect.getAmplifier());
                    })) {
                        type.removeAttributeModifiers(attributes);
                        iterator.remove();
                    }
                }
                var movement = attributes.getInstance(Attributes.MOVEMENT_SPEED);
                var row = new JsonObject();
                row.addProperty("value", movement.getValue());
                for (String name : List.of("speed", "slowness")) {
                    var modifier = movement.getModifier(Identifier.parse("minecraft:effect." + name));
                    if (modifier != null) row.addProperty(name, modifier.amount());
                }
                rows.add(row);
            }
            output.add(rows);
        }
        Files.writeString(Path.of(args[2]), new Gson().toJson(output));
    }
}
