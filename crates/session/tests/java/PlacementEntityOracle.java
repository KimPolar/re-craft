import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.entity.ExperienceOrb;
import net.minecraft.world.entity.item.ItemEntity;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.entity.decoration.ArmorStand;

public class PlacementEntityOracle {
    static class World extends ServerLevel {
        World() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public int getNextEntityId() { return 1; }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe = (sun.misc.Unsafe)field.get(null);
        var world = (World)unsafe.allocateInstance(World.class);
        var result = new JsonObject();
        result.addProperty("item",new ItemEntity(EntityTypes.ITEM,world).blocksBuilding);
        result.addProperty("experience_orb",new ExperienceOrb(EntityTypes.EXPERIENCE_ORB,world).blocksBuilding);
        result.addProperty("armor_stand",new ArmorStand(EntityTypes.ARMOR_STAND,world).blocksBuilding);
        result.addProperty("loaded_falling_block",new FallingBlockEntity(EntityTypes.FALLING_BLOCK,world).blocksBuilding);
        Files.writeString(Path.of(args[0]),result.toString());
    }
}
