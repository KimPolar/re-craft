import com.google.gson.*;
import java.lang.reflect.Modifier;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.entity.EntityTypes;

public class EntityCategoryOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var result=new JsonObject();
        for (var field:EntityTypes.class.getFields()) {
            if (!Modifier.isStatic(field.getModifiers()) || field.getType()!=EntityType.class) continue;
            var type=(EntityType<?>)field.get(null);
            var id=BuiltInRegistries.ENTITY_TYPE.getKey(type);
            if (id==null) throw new AssertionError(field);
            result.addProperty(id.toString(),type.getCategory().getSerializedName());
        }
        Files.writeString(Path.of(args[0]),result.toString());
    }
}
