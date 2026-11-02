import com.google.gson.*;
import com.mojang.brigadier.StringReader;
import java.nio.file.*;
import net.minecraft.commands.arguments.GameModeArgument;
import net.minecraft.world.entity.player.Abilities;
import net.minecraft.world.level.GameType;
import net.minecraft.world.entity.Entity;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.level.ServerPlayer;
import net.minecraft.server.level.ServerPlayerGameMode;
import net.minecraft.world.level.border.WorldBorder;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.Shapes;
import net.minecraft.world.phys.shapes.VoxelShape;
import java.util.*;

public class GameModeOracle {
    static class Border extends WorldBorder {
        @Override public boolean isInsideCloseToBorder(Entity entity, AABB area) { return false; }
    }
    static class GroundWorld extends ServerLevel {
        WorldBorder border; boolean obstructed;
        GroundWorld() { super(null,null,null,null,null,null,false,0L,List.of(),false); }
        @Override public WorldBorder getWorldBorder() { return border; }
        @Override public List<VoxelShape> getEntityCollisions(Entity entity, AABB area) { return List.of(); }
        @Override public Iterable<VoxelShape> getBlockCollisions(Entity entity, AABB area) {
            var shapes = new ArrayList<VoxelShape>();
            var floor = Shapes.box(0,0,0,1,1,1);
            if (floor.bounds().intersects(area)) shapes.add(floor);
            var obstruction = Shapes.box(0,1,0,1,2,1);
            if (obstructed && obstruction.bounds().intersects(area)) shapes.add(obstruction);
            return shapes;
        }
    }
    static void field(Class<?> owner, Object target, String name, Object value) throws Exception {
        var f = owner.getDeclaredField(name); f.setAccessible(true); f.set(target, value);
    }
    public static void main(String[] args) throws Exception {
        var rows = new JsonArray();
        for (var name : new String[]{"survival", "creative", "adventure", "spectator", "Creative", "1", "spectatorx", "", "survival:foo"}) {
            var reader = new StringReader(name);
            boolean valid;
            try { GameModeArgument.gameMode().parse(reader); valid = !reader.canRead(); }
            catch (Exception e) { valid = false; }
            var row = new JsonObject(); row.addProperty("input", name); row.addProperty("valid", valid); rows.add(row);
        }
        var abilities = new JsonArray();
        for (var target : GameType.values()) for (boolean initial : new boolean[]{false, true}) {
            var value = new Abilities(); value.flying = initial; target.updatePlayerAbilities(value);
            var row = new JsonObject(); row.addProperty("mode", target.getName()); row.addProperty("initial", initial); row.addProperty("flying", value.flying); abilities.add(row);
        }
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        var world=(GroundWorld)unsafe.allocateInstance(GroundWorld.class); world.border=new Border();
        var player=(ServerPlayer)unsafe.allocateInstance(ServerPlayer.class);
        field(Entity.class,player,"level",world);
        var gameMode=(ServerPlayerGameMode)unsafe.allocateInstance(ServerPlayerGameMode.class);
        field(ServerPlayerGameMode.class,gameMode,"player",player); field(ServerPlayerGameMode.class,gameMode,"level",world);
        var probe=ServerPlayerGameMode.class.getDeclaredMethod("isInRangeOfGround"); probe.setAccessible(true);
        var ground=new JsonArray();
        for (boolean blocked : new boolean[]{false,true}) for (double y : new double[]{1.0,1.25,1.9999,2.0,2.01,3.0}) {
            world.obstructed=blocked;
            player.setBoundingBox(new AABB(0.5-(double)(0.6f/2),y,0.5-(double)(0.6f/2),0.5+(double)(0.6f/2),y+(double)1.8f,0.5+(double)(0.6f/2)));
            var row=new JsonObject(); row.addProperty("y",y); row.addProperty("blocked",blocked); row.addProperty("near",(boolean)probe.invoke(gameMode)); ground.add(row);
        }
        var result = new JsonObject(); result.add("arguments", rows); result.add("abilities", abilities); result.add("ground",ground);
        Files.writeString(Path.of(args[0]), result.toString());
    }
}
