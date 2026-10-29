import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import java.util.function.Predicate;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.PlayerSpawnFinder;
import net.minecraft.core.BlockPos;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityDimensions;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.level.EntityGetter;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.Vec3;

/** Exercise the real EntityGetter collision predicate for synthetic entities. */
public final class WorldSpawnEntityOracle {
    static final class World implements InvocationHandler {
        final List<Entity> entities = new ArrayList<>();
        @Override public Object invoke(Object proxy, Method method, Object[] args) throws Throwable {
            if (method.getName().equals("getEntities") && args.length == 3) {
                AABB box = (AABB)args[1];
                @SuppressWarnings("unchecked") Predicate<Entity> predicate = (Predicate<Entity>)args[2];
                return entities.stream().filter(entity -> entity.getBoundingBox().intersects(box) && predicate.test(entity)).toList();
            }
            if (method.isDefault()) return InvocationHandler.invokeDefault(proxy, method, args);
            throw new UnsupportedOperationException(method.toString());
        }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field unsafeField = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)unsafeField.get(null);
        Field dimensionsField = PlayerSpawnFinder.class.getDeclaredField("PLAYER_DIMENSIONS"); dimensionsField.setAccessible(true);
        EntityDimensions dimensions = (EntityDimensions)dimensionsField.get(null);
        AABB player = dimensions.makeBoundingBox(Vec3.atBottomCenterOf(new BlockPos(8, 1, 8)));
        JsonArray ids = JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
        JsonArray results = new JsonArray();
        for (JsonElement element : ids) {
            String id = element.getAsString();
            Field typeField = EntityTypes.class.getField(id.toUpperCase(Locale.ROOT));
            Class<?> entityClass = (Class<?>)((ParameterizedType)typeField.getGenericType()).getActualTypeArguments()[0];
            Entity entity = (Entity)unsafe.allocateInstance(entityClass);
            entity.setBoundingBox(new AABB(8.0, 1.0, 8.0, 9.0, 2.0, 9.0));
            World world = new World(); world.entities.add(entity);
            EntityGetter level = (EntityGetter)Proxy.newProxyInstance(
                WorldSpawnEntityOracle.class.getClassLoader(), new Class<?>[]{EntityGetter.class}, world);
            results.add(level.getEntityCollisions(null, player).isEmpty());
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(results));
    }
}
