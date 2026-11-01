import java.lang.reflect.*;
import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.client.server.IntegratedServer;
import net.minecraft.core.Holder;
import net.minecraft.network.protocol.Packet;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtIo;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtOps;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.level.ServerLevel;
import net.minecraft.server.players.PlayerList;
import net.minecraft.util.RandomSource;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.gamerules.GameRule;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.saveddata.WeatherData;
import net.minecraft.commands.CommandSourceStack;
import net.minecraft.server.MinecraftServer;
import net.minecraft.world.entity.Entity;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.contents.TranslatableContents;
import net.minecraft.server.commands.WeatherCommand;

public class WeatherOracle {
    static class Source extends CommandSourceStack {
        String message;
        Source() { super(null,null,null,null,null,(MinecraftServer)null,(Entity)null); }
        @Override public void sendSuccess(java.util.function.Supplier<Component> supplier, boolean broadcast) {
            if (!broadcast) throw new AssertionError("weather message should broadcast");
            message = ((TranslatableContents)supplier.get().getContents()).getKey();
        }
    }
    static class Rules extends GameRules {
        boolean enabled;
        Rules() { super((FeatureFlagSet)null); }
        @SuppressWarnings("unchecked") @Override public <T> T get(GameRule<T> rule) { return (T)Boolean.valueOf(enabled); }
    }
    static class Players extends PlayerList {
        Players() { super(null,null,null,null); }
        @Override public void broadcastAll(Packet<?> packet) {}
        @Override public void broadcastAll(Packet<?> packet, ResourceKey<Level> dimension) {}
    }
    static class Server extends IntegratedServer {
        PlayerList list;
        Server() { super(null,null,null,null,null,Optional.empty(),null,null); }
        @Override public PlayerList getPlayerList() { return list; }
    }
    static class World extends ServerLevel {
        WeatherData weather; Rules rules; boolean canWeather;
        World() { super(null,null,null,null,null,null,false,0,List.of(),false); }
        @Override public WeatherData getWeatherData() { return weather; }
        @Override public GameRules getGameRules() { return rules; }
        @Override public boolean canHaveWeather() { return canWeather; }
        @Override public ResourceKey<Level> dimension() { return Level.OVERWORLD; }
    }
    static Field field(Class<?> c,String name) throws Exception { var f=c.getDeclaredField(name); f.setAccessible(true); return f; }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var rustDocument = NbtIo.readCompressed(Path.of(args[2]), NbtAccounter.unlimitedHeap());
        var rustWeather = WeatherData.CODEC.parse(NbtOps.INSTANCE, rustDocument.get("data")).getOrThrow();
        if (rustWeather.getClearWeatherTime() != 37 || rustWeather.getRainTime() != 239 || rustWeather.getThunderTime() != 919 || !rustWeather.isRaining() || rustWeather.isThundering()) throw new AssertionError("Rust weather document differs");
        var levelDocument = NbtIo.readCompressed(Path.of(args[3]), NbtAccounter.unlimitedHeap());
        var levelData = new com.mojang.serialization.Dynamic<>(NbtOps.INSTANCE, levelDocument.get("Data"));
        if (levelData.get("Time").asLong(0) != -9223372036854775798L) throw new AssertionError("Rust level time differs");
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var u=(sun.misc.Unsafe)f.get(null);
        var world=(World)u.allocateInstance(World.class); var rules=(Rules)u.allocateInstance(Rules.class);
        var server=(Server)u.allocateInstance(Server.class); server.list=(Players)u.allocateInstance(Players.class);
        field(ServerLevel.class,"server").set(world,server); world.rules=rules;
        var randomField=field(Level.class,"random"); var rain=field(Level.class,"rainLevel"); var thunder=field(Level.class,"thunderLevel");
        var oldRain=field(Level.class,"oRainLevel"); var oldThunder=field(Level.class,"oThunderLevel");
        var tick=ServerLevel.class.getDeclaredMethod("advanceWeatherCycle"); tick.setAccessible(true);
        var random=new Random(19773); var result=new JsonArray();
        for(int n=0;n<240;n++) {
            long seed=random.nextLong(); boolean can=random.nextBoolean(), advance=random.nextBoolean();
            boolean raining=random.nextBoolean(), thundering=random.nextBoolean();
            int[] times={-1,0,1,2,3,12000};
            int clear=times[random.nextInt(times.length)], rt=times[random.nextInt(times.length)], tt=times[random.nextInt(times.length)];
            world.weather=new WeatherData(clear,rt,tt,raining,thundering); world.canWeather=can; rules.enabled=advance;
            var rng=RandomSource.create(seed); randomField.set(world,rng);
            rain.setFloat(world,raining?1f:0f); thunder.setFloat(world,raining&&thundering?1f:0f);
            oldRain.setFloat(world,0f); oldThunder.setFloat(world,0f);
            var trace=new JsonArray();
            for(int i=0;i<110;i++) {
                tick.invoke(world);
                var row=new JsonArray(); row.add(world.weather.getClearWeatherTime()); row.add(world.weather.getRainTime());
                row.add(world.weather.getThunderTime()); row.add(world.weather.isRaining()); row.add(world.weather.isThundering());
                row.add(Float.floatToIntBits(rain.getFloat(world))); row.add(Float.floatToIntBits(thunder.getFloat(world)));
                row.add(world.isRaining()); row.add(world.isThundering()); trace.add(row);
            }
            var item=new JsonObject(); item.addProperty("seed",seed); item.addProperty("can",can); item.addProperty("advance",advance);
            item.addProperty("raining",raining); item.addProperty("thundering",thundering);
            item.addProperty("clear",clear); item.addProperty("rain",rt); item.addProperty("thunder",tt); item.addProperty("next",rng.nextInt());
            item.add("trace",trace); result.add(item);
        }
        Files.writeString(Path.of(args[0]),result.toString());
        var source=(Source)u.allocateInstance(Source.class);
        field(CommandSourceStack.class,"server").set(source,server);
        field(CommandSourceStack.class,"level").set(source,world);
        var commands=new JsonArray();
        for(String mode:new String[]{"clear","rain","thunder"}) {
            String methodName="set"+Character.toUpperCase(mode.charAt(0))+mode.substring(1);
            var method=WeatherCommand.class.getDeclaredMethod(methodName,CommandSourceStack.class,int.class);
            method.setAccessible(true);
            for(int duration:new int[]{-1,1,40,Integer.MAX_VALUE}) for(long seed:new long[]{0,1,-1,123456789,Long.MIN_VALUE,Long.MAX_VALUE}) {
                var data=new WeatherData(37,239,919,true,true);
                field(MinecraftServer.class,"weatherData").set(server,data);
                var rng=RandomSource.create(seed); randomField.set(world,rng);
                int returned=(int)method.invoke(null,source,duration);
                var row=new JsonObject(); row.addProperty("mode",mode); row.addProperty("duration",duration); row.addProperty("seed",seed);
                row.addProperty("clear",data.getClearWeatherTime()); row.addProperty("rain",data.getRainTime()); row.addProperty("thunder",data.getThunderTime());
                row.addProperty("raining",data.isRaining()); row.addProperty("thundering",data.isThundering());
                row.addProperty("next",rng.nextInt()); row.addProperty("returned",returned); row.addProperty("message",source.message); commands.add(row);
            }
        }
        Files.writeString(Path.of(args[4]),commands.toString());
        var saved=new WeatherData(37,239,919,true,false);
        var document=new CompoundTag();
        document.putInt("DataVersion",5023);
        document.put("data",WeatherData.CODEC.encodeStart(NbtOps.INSTANCE,saved).getOrThrow());
        NbtIo.writeCompressed(document,Path.of(args[1]));
    }
}
