import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import java.lang.reflect.Field;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import net.minecraft.client.gui.components.AbstractWidget;
import net.minecraft.client.gui.components.EditBox;
import net.minecraft.client.gui.Font;
import net.minecraft.client.StringSplitter;
import net.minecraft.client.input.MouseButtonEvent;
import net.minecraft.client.input.MouseButtonInfo;
import com.mojang.brigadier.CommandDispatcher;
import net.minecraft.commands.CommandSourceStack;
import net.minecraft.server.commands.SeedCommand;
import net.minecraft.server.permissions.PermissionSet;
import net.minecraft.network.chat.Component;
import net.minecraft.network.chat.ComponentUtils;
import net.minecraft.network.chat.ClickEvent;
import net.minecraft.network.chat.contents.TranslatableContents;
import net.minecraft.commands.arguments.TimeArgument;
import net.minecraft.commands.arguments.IdentifierArgument;
import com.mojang.brigadier.StringReader;
import net.minecraft.world.clock.ClockTimeMarker;
import java.util.Optional;
import net.minecraft.world.clock.ServerClockManager;
import net.minecraft.core.MappedRegistry;
import net.minecraft.core.Registry;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.registries.Registries;
import net.minecraft.world.clock.WorldClock;
import net.minecraft.world.timeline.Timeline;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.commands.CommandBuildContext;
import net.minecraft.server.commands.TimeCommand;
import net.minecraft.server.commands.WeatherCommand;
import com.mojang.serialization.Lifecycle;
import com.mojang.serialization.JsonOps;
import com.google.gson.JsonParser;
import net.minecraft.client.input.KeyEvent;

// Compare the real 26.3 EditBox input state without creating a game window.
public final class ChatEditOracle {
    static final class WaterPlayer extends net.minecraft.client.player.LocalPlayer {
        boolean underwater;
        WaterPlayer() { super(null, null, null, null, null, null, false, null, null); }
        @Override public boolean isEyeInFluid(net.minecraft.tags.TagKey<net.minecraft.world.level.material.Fluid> tag) { return underwater; }
    }
    static final class FogProbe extends net.minecraft.world.attribute.EnvironmentAttributeProbe {
        float start, end; org.joml.Vector3f color;
        @Override @SuppressWarnings("unchecked") public <T> T getValue(net.minecraft.world.attribute.EnvironmentAttribute<T> key, float partial) {
            var attrs = net.minecraft.world.attribute.EnvironmentAttributes.class;
            if (key == net.minecraft.world.attribute.EnvironmentAttributes.WATER_FOG_START_DISTANCE) return (T) Float.valueOf(start);
            if (key == net.minecraft.world.attribute.EnvironmentAttributes.WATER_FOG_END_DISTANCE) return (T) Float.valueOf(end);
            if (key == net.minecraft.world.attribute.EnvironmentAttributes.WATER_FOG_COLOR) return (T) color;
            if (key == net.minecraft.world.attribute.EnvironmentAttributes.FOG_COLOR || key == net.minecraft.world.attribute.EnvironmentAttributes.SKY_COLOR) return (T) color;
            if (key == net.minecraft.world.attribute.EnvironmentAttributes.SKY_FOG_END_DISTANCE) return (T) Float.valueOf(512f);
            throw new AssertionError(key);
        }
    }
    static final class FogCamera extends net.minecraft.client.Camera {
        FogProbe probe; WaterPlayer player; double y; boolean water;
        @Override public net.minecraft.world.attribute.EnvironmentAttributeProbe attributeProbe() { return probe; }
        @Override public net.minecraft.world.entity.Entity entity() { return player; }
        @Override public net.minecraft.world.level.material.FogType getFluidInCamera() { return water ? net.minecraft.world.level.material.FogType.WATER : net.minecraft.world.level.material.FogType.NONE; }
        @Override public net.minecraft.world.phys.Vec3 position() { return new net.minecraft.world.phys.Vec3(0, y, 0); }
    }
    static final class FogLevel extends net.minecraft.client.multiplayer.ClientLevel {
        net.minecraft.client.multiplayer.ClientLevel.ClientLevelData data;
        FogLevel() { super(null, null, null, null, 0, 0, null, false, 0, 0); }
        @Override public int getMinY() { return -64; }
        @Override public net.minecraft.client.multiplayer.ClientLevel.ClientLevelData getLevelData() { return data; }
        @Override public float getRainLevel(float partial) { return 0f; }
        @Override public float getThunderLevel(float partial) { return 0f; }
    }
    static void set(Object object, Class<?> owner, String name, Object value) throws Exception {
        Field field = owner.getDeclaredField(name);
        field.setAccessible(true);
        field.set(object, value);
    }
    static int get(EditBox box, String name) throws Exception {
        Field field = EditBox.class.getDeclaredField(name);
        field.setAccessible(true);
        return field.getInt(box);
    }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion();
        net.minecraft.server.Bootstrap.bootStrap();
        Field field = sun.misc.Unsafe.class.getDeclaredField("theUnsafe");
        field.setAccessible(true);
        sun.misc.Unsafe unsafe = (sun.misc.Unsafe) field.get(null);
        CommandSourceStack source = (CommandSourceStack) unsafe.allocateInstance(CommandSourceStack.class);
        for (boolean dedicated : new boolean[]{false, true}) {
            CommandDispatcher<CommandSourceStack> dispatcher = new CommandDispatcher<>();
            SeedCommand.register(dispatcher, dedicated);
            for (boolean allowed : new boolean[]{false, true}) {
                set(source, CommandSourceStack.class, "permissions", allowed ? PermissionSet.ALL_PERMISSIONS : PermissionSet.NO_PERMISSIONS);
                if (dispatcher.getRoot().getChild("seed").canUse(source) != (!dedicated || allowed))
                    throw new AssertionError("seed permissions");
            }
        }
        String seed = "-9223372036854775808";
        Component wrapped = ComponentUtils.copyOnClickText(seed);
        TranslatableContents bracket = (TranslatableContents) wrapped.getContents();
        Component digit = (Component) bracket.getArgs()[0];
        if (!bracket.getKey().equals("chat.square_brackets") ||
            digit.getStyle().getColor().getValue() != 0x55ff55 ||
            !(digit.getStyle().getClickEvent() instanceof ClickEvent.CopyToClipboard copy) ||
            !copy.value().equals(seed) || !seed.equals(digit.getStyle().getInsertion()) ||
            digit.getStyle().getHoverEvent() == null || digit.getStyle().isUnderlined())
            throw new AssertionError("seed component style");
        String[] texts = {"", "abc  def", " a   b ", "한글  😀-two  끝", "a\u00a0b c", "a\tb c"};
        int[] codes = {1073741904, 1073741903, 8, 127};
        JsonArray rows = new JsonArray();
        var waterPlayer = (WaterPlayer) unsafe.allocateInstance(WaterPlayer.class);
        for (int time = 0; time <= 620; time++) for (boolean underwater : new boolean[]{false, true}) {
            set(waterPlayer, net.minecraft.client.player.LocalPlayer.class, "waterVisionTime", time);
            waterPlayer.underwater = underwater;
            JsonObject row = new JsonObject();
            row.addProperty("waterVisionTime", time); row.addProperty("waterEye", underwater);
            row.addProperty("waterVisionBits", Float.floatToRawIntBits(waterPlayer.getWaterVision())); rows.add(row);
        }
        var fogCamera = (FogCamera) unsafe.allocateInstance(FogCamera.class);
        fogCamera.player = waterPlayer;
        fogCamera.y = 100;
        fogCamera.water = true;
        fogCamera.probe = (FogProbe) unsafe.allocateInstance(FogProbe.class);
        var waterEnvironment = new net.minecraft.client.renderer.fog.environment.WaterFogEnvironment();
        for (int time : new int[]{0, 1, 25, 100, 350, 599, 600}) for (boolean underwater : new boolean[]{false, true})
        for (float start : new float[]{-8f, 0f, 15f}) for (float end : new float[]{0f, 24f, 96f, 1024f}) {
            set(waterPlayer, net.minecraft.client.player.LocalPlayer.class, "waterVisionTime", time); waterPlayer.underwater = underwater;
            fogCamera.probe.start = start; fogCamera.probe.end = end;
            var data = new net.minecraft.client.renderer.fog.FogData();
            waterEnvironment.setupFog(data, fogCamera, null, 0f, net.minecraft.client.DeltaTracker.ZERO);
            JsonObject row = new JsonObject(); row.addProperty("waterFogTime", time); row.addProperty("waterEye", underwater);
            row.addProperty("waterFogStart", start); row.addProperty("waterFogEnd", end);
            row.addProperty("waterFogStartBits", Float.floatToRawIntBits(data.environmentalStart));
            row.addProperty("waterFogEndBits", Float.floatToRawIntBits(data.environmentalEnd)); rows.add(row);
        }
        var fogLevel = (FogLevel) unsafe.allocateInstance(FogLevel.class);
        fogLevel.data = (net.minecraft.client.multiplayer.ClientLevel.ClientLevelData) unsafe.allocateInstance(net.minecraft.client.multiplayer.ClientLevel.ClientLevelData.class);
        var playerEffects = new java.util.HashMap<net.minecraft.core.Holder<net.minecraft.world.effect.MobEffect>, net.minecraft.world.effect.MobEffectInstance>();
        set(waterPlayer, net.minecraft.world.entity.LivingEntity.class, "activeEffects", playerEffects);
        var fogRenderer = (net.minecraft.client.renderer.fog.FogRenderer) unsafe.allocateInstance(net.minecraft.client.renderer.fog.FogRenderer.class);
        var computeFog = net.minecraft.client.renderer.fog.FogRenderer.class.getDeclaredMethod("computeFogColor", net.minecraft.client.Camera.class, float.class, net.minecraft.client.multiplayer.ClientLevel.class, int.class, float.class, org.joml.Vector4f.class);
        computeFog.setAccessible(true);
        for (int time : new int[]{0, 1, 25, 100, 350, 599, 600}) for (boolean underwater : new boolean[]{false, true})
        for (float[] color : new float[][]{{5f/255f,5f/255f,51f/255f},{0.2f,0.4f,0.6f},{0f,0.3f,0.5f},{1f,1f,1f}}) {
            set(waterPlayer, net.minecraft.client.player.LocalPlayer.class, "waterVisionTime", time); waterPlayer.underwater = underwater;
            fogCamera.probe.color = new org.joml.Vector3f(color[0],color[1],color[2]);
            var result = new org.joml.Vector4f(); computeFog.invoke(fogRenderer, fogCamera, 0f, fogLevel, 12, 0f, result);
            JsonObject row = new JsonObject(); row.addProperty("waterFogColorTime", time); row.addProperty("waterEye", underwater);
            row.addProperty("r",color[0]); row.addProperty("g",color[1]); row.addProperty("b",color[2]);
            row.addProperty("rBits",Float.floatToRawIntBits(result.x)); row.addProperty("gBits",Float.floatToRawIntBits(result.y)); row.addProperty("bBits",Float.floatToRawIntBits(result.z)); rows.add(row);
        }
        for (boolean flat : new boolean[]{false, true}) for (double y : new double[]{-100, -64, -63.75, -63, -48, -32, -31, 0, 100})
        for (int time : new int[]{0, 50, 100, 350, 600}) for (boolean underwater : new boolean[]{false, true}) {
            set(fogLevel.data, net.minecraft.client.multiplayer.ClientLevel.ClientLevelData.class, "isFlat", flat);
            set(waterPlayer, net.minecraft.client.player.LocalPlayer.class, "waterVisionTime", time); waterPlayer.underwater = underwater;
            fogCamera.y = y; fogCamera.probe.color = new org.joml.Vector3f(0.2f, 0.4f, 0.6f);
            var result = new org.joml.Vector4f(); computeFog.invoke(fogRenderer, fogCamera, 0f, fogLevel, 12, 0f, result);
            JsonObject row = new JsonObject(); row.addProperty("voidFogTime", time); row.addProperty("waterEye", underwater); row.addProperty("flat", flat); row.addProperty("y", y);
            row.addProperty("rBits",Float.floatToRawIntBits(result.x)); row.addProperty("gBits",Float.floatToRawIntBits(result.y)); row.addProperty("bBits",Float.floatToRawIntBits(result.z)); rows.add(row);
        }
        fogCamera.water = false;
        for (boolean flat : new boolean[]{false, true}) for (double y : new double[]{-64, -63.75, -48, -32, 100})
        for (int duration : new int[]{-2, -1, 50, 200, 201}) for (float partial : new float[]{0f, .5f, 1f}) {
            set(fogLevel.data, net.minecraft.client.multiplayer.ClientLevel.ClientLevelData.class, "isFlat", flat);
            playerEffects.clear();
            if (duration != -2) playerEffects.put(net.minecraft.world.effect.MobEffects.NIGHT_VISION, new net.minecraft.world.effect.MobEffectInstance(net.minecraft.world.effect.MobEffects.NIGHT_VISION, duration));
            fogCamera.y = y; fogCamera.probe.color = new org.joml.Vector3f(0.2f, 0.4f, 0.6f);
            var result = new org.joml.Vector4f(); computeFog.invoke(fogRenderer, fogCamera, partial, fogLevel, 2, 0f, result);
            JsonObject row = new JsonObject(); row.addProperty("airFogDuration", duration); row.addProperty("partial", partial); row.addProperty("flat", flat); row.addProperty("y", y);
            row.addProperty("rBits",Float.floatToRawIntBits(result.x)); row.addProperty("gBits",Float.floatToRawIntBits(result.y)); row.addProperty("bBits",Float.floatToRawIntBits(result.z)); rows.add(row);
        }
        for (String text : texts) {
            ArrayList<Integer> positions = new ArrayList<>();
            for (int at = 0; ; at = text.offsetByCodePoints(at, 1)) {
                positions.add(at);
                if (at == text.length()) break;
            }
            for (int cursor : positions) for (int anchor : positions)
            for (int code : codes) for (int modifiers : new int[]{0, 1, 64, 65}) {
                EditBox box = (EditBox) unsafe.allocateInstance(EditBox.class);
                set(box, EditBox.class, "value", text);
                set(box, EditBox.class, "maxLength", 256);
                set(box, EditBox.class, "cursorPos", cursor);
                set(box, EditBox.class, "highlightPos", anchor);
                set(box, EditBox.class, "isEditable", true);
                set(box, AbstractWidget.class, "active", true);
                set(box, AbstractWidget.class, "visible", true);
                set(box, AbstractWidget.class, "focused", true);
                box.keyPressed(new KeyEvent(0, code, modifiers));
                JsonObject row = new JsonObject();
                row.addProperty("before", text);
                row.addProperty("cursor", cursor);
                row.addProperty("anchor", anchor);
                row.addProperty("code", code);
                row.addProperty("modifiers", modifiers);
                row.addProperty("after", box.getValue());
                row.addProperty("afterCursor", get(box, "cursorPos"));
                row.addProperty("afterAnchor", get(box, "highlightPos"));
                rows.add(row);
            }
        }
        Font font = (Font) unsafe.allocateInstance(Font.class);
        set(font, Font.class, "splitter", new StringSplitter((codepoint, style) -> 1.0f));
        for (String text : texts) {
            int count = text.codePointCount(0, text.length());
            for (int x = 0; x <= count + 1; x++) for (boolean twice : new boolean[]{false, true}) {
                EditBox box = (EditBox) unsafe.allocateInstance(EditBox.class);
                set(box, EditBox.class, "value", text);
                set(box, EditBox.class, "font", font);
                set(box, AbstractWidget.class, "width", 256);
                box.onClick(new MouseButtonEvent(x, 0, new MouseButtonInfo(1, 0)), twice);
                JsonObject row = new JsonObject();
                row.addProperty("before", text);
                row.addProperty("clickX", x);
                row.addProperty("double", twice);
                row.addProperty("afterCursor", get(box, "cursorPos"));
                row.addProperty("afterAnchor", get(box, "highlightPos"));
                rows.add(row);
            }
        }
        ArrayList<String> times = new ArrayList<>();
        for (int n = -200; n <= 200; n++) times.add(Float.toString(n / 10.0f));
        for (String special : new String[]{"", "-", ".", "+1", "1e3", "-0.5", "-0.00001", "2147483647", "2147483648", "999999999999999999999999999999999999999999", "1.2.3", "--1"}) times.add(special);
        for (String time : times) for (String unit : new String[]{"", "t", "s", "d", "x"}) for (int minimum : new int[]{0, Integer.MIN_VALUE}) {
            String input = time + unit;
            JsonObject row = new JsonObject();
            row.addProperty("timeArgument", input);
            row.addProperty("minimum", minimum);
            try {
                StringReader reader = new StringReader(input);
                int ticks = TimeArgument.time(minimum).parse(reader);
                if (reader.canRead()) throw new IllegalArgumentException("trailing characters");
                row.addProperty("ticks", ticks);
            } catch (Exception ignored) {
                row.add("ticks", com.google.gson.JsonNull.INSTANCE);
            }
            rows.add(row);
        }
        for (int period : new int[]{0, 1, 48, 24000, Integer.MAX_VALUE})
        for (int marker : new int[]{0, 1, 5, 47, 1000, 18000, Integer.MAX_VALUE})
        for (long current : new long[]{Long.MIN_VALUE, -24001, -1, 0, 1, 5, 47, 48, 1000, 23999, 24000, Long.MAX_VALUE}) {
            ClockTimeMarker definition = new ClockTimeMarker(null, marker, period == 0 ? Optional.empty() : Optional.of(period), true);
            JsonObject row = new JsonObject();
            row.addProperty("markerTicks", marker);
            row.addProperty("markerPeriod", period);
            row.addProperty("markerCurrent", current);
            if (definition.occursAt(current)) row.add("markerTarget", com.google.gson.JsonNull.INSTANCE);
            else row.addProperty("markerTarget", definition.resolveTimeToMoveTo(current));
            rows.add(row);
        }
        Class<?> instanceType = ServerClockManager.ServerClockInstance.class;
        var addTicks = ServerClockManager.class.getDeclaredMethod("lambda$addTicks$0", int.class, instanceType);
        var setTicks = ServerClockManager.class.getDeclaredMethod("lambda$setTotalTicks$0", long.class, instanceType);
        addTicks.setAccessible(true);
        setTicks.setAccessible(true);
        for (long initial : new long[]{Long.MIN_VALUE, -1, 0, 1, 1000, Long.MAX_VALUE})
        for (int amount : new int[]{Integer.MIN_VALUE, -1, 0, 1, 1000, Integer.MAX_VALUE})
        for (boolean add : new boolean[]{false, true}) {
            var clock = new ServerClockManager.ServerClockInstance();
            set(clock, instanceType, "totalTicks", initial);
            set(clock, instanceType, "partialTick", 0.75f);
            if (add) addTicks.invoke(null, amount, clock);
            else if (initial != amount) setTicks.invoke(null, (long) amount, clock);
            Field total = instanceType.getDeclaredField("totalTicks");
            Field partial = instanceType.getDeclaredField("partialTick");
            total.setAccessible(true); partial.setAccessible(true);
            JsonObject row = new JsonObject();
            row.addProperty("clockInitial", initial);
            row.addProperty("clockAmount", amount);
            row.addProperty("clockAdd", add);
            row.addProperty("clockTotal", total.getLong(clock));
            row.addProperty("clockPartial", partial.getFloat(clock));
            rows.add(row);
        }
        for (String prefix : new String[]{"", ":", "minecraft:", "example:", "example_pack:", "Bad:"})
        for (String suffix : new String[]{"day", "wake", "path/to/marker", "clock_2", "my-marker", "bad?", "UPPER", ""}) {
            String input = prefix + suffix;
            JsonObject row = new JsonObject();
            row.addProperty("identifierArgument", input);
            try {
                StringReader reader = new StringReader(input);
                var id = IdentifierArgument.id().parse(reader);
                if (reader.canRead()) throw new IllegalArgumentException("trailing characters");
                row.addProperty("identifierResult", id.toString());
            } catch (Exception ignored) {
                row.add("identifierResult", com.google.gson.JsonNull.INSTANCE);
            }
            rows.add(row);
        }
        var clocks = new MappedRegistry<WorldClock>(Registries.WORLD_CLOCK, Lifecycle.stable());
        Registry.register(clocks, "example:clock", new WorldClock());
        clocks.freeze();
        var timelines = new MappedRegistry<Timeline>(Registries.TIMELINE, Lifecycle.stable());
        var timelineConstructor = Timeline.class.getDeclaredConstructor(net.minecraft.core.Holder.class, Optional.class, java.util.Map.class, java.util.Map.class);
        timelineConstructor.setAccessible(true);
        var clockHolder = clocks.getOrThrow(net.minecraft.resources.ResourceKey.create(Registries.WORLD_CLOCK, net.minecraft.resources.Identifier.parse("example:clock")));
        Registry.register(timelines, "example:timeline", timelineConstructor.newInstance(clockHolder, Optional.of(48), java.util.Map.of(), java.util.Map.of()));
        Registry.register(timelines, "minecraft:gametime", timelineConstructor.newInstance(clockHolder, Optional.empty(), java.util.Map.of(), java.util.Map.of()));
        timelines.freeze();
        var lookup = HolderLookup.Provider.create(java.util.stream.Stream.of(clocks, timelines));
        var commandContext = CommandBuildContext.simple(lookup, FeatureFlags.DEFAULT_FLAGS);
        var commands = new CommandDispatcher<CommandSourceStack>();
        TimeCommand.register(commands, commandContext);
        set(source, CommandSourceStack.class, "permissions", PermissionSet.ALL_PERMISSIONS);
        for (String command : new String[]{"time of example:clock set example:wake", "time of example:clock set 1d", "time of example:clock set -1", "time of example:clock query time", "time of example:clock query gametime", "time of example:clock query example:missing", "time of example:clock add -1.5s", "time of example:clock add nope", "time of example:clock pause", "time of example:clock resume", "time of example:clock query time extra", "time set example:wake", "time query gametime", "time set +1", "time set example:", "time rate 0.00001", "time rate 1000", "time rate 1000.1", "time rate 0", "time rate -1", "time rate 1e2", "time rate +1", "time rate 1.5", "time of example:clock rate .5", "time query example:timeline", "time of example:clock query example:timeline repetition", "time query example:timeline repetitions"}) {
            var parsed = commands.parse(command, source);
            JsonObject row = new JsonObject();
            row.addProperty("timeSyntax", "/" + command);
            row.addProperty("timeSyntaxAccepted", !parsed.getReader().canRead() && parsed.getContext().getCommand() != null);
            rows.add(row);
        }
        for (int period : new int[]{0, 1, 48, 24000, Integer.MAX_VALUE})
        for (long ticks : new long[]{Long.MIN_VALUE, -2147483649L, -49, -1, 0, 1, 47, 48, 49, 2147483648L, Long.MAX_VALUE}) {
            var timeline = timelineConstructor.newInstance(clockHolder, period == 0 ? Optional.empty() : Optional.of(period), java.util.Map.of(), java.util.Map.of());
            net.minecraft.world.clock.ClockInstance instance = new net.minecraft.world.clock.ClockInstance() {
                public long totalTicks() { return ticks; }
                public float partialTick() { return 0; }
                public float rate() { return 1; }
                public boolean isPaused() { return false; }
            };
            net.minecraft.world.clock.ClockManager manager = holder -> instance;
            JsonObject row = new JsonObject();
            row.addProperty("timelinePeriod", period);
            row.addProperty("timelineTicks", ticks);
            row.addProperty("timelineCurrent", timeline.getCurrentTicks(manager));
            row.addProperty("timelineRepetitions", timeline.getPeriodCount(manager));
            rows.add(row);
        }
        for (String rate : new String[]{"0.00001", "0.00002", "0.0001", "0.000999", "0.001", "0.1", "1", "1.5", "10", "1000", "1.2345678"}) {
            JsonObject row = new JsonObject();
            row.addProperty("rateValue", rate);
            row.addProperty("rateString", Float.toString(Float.parseFloat(rate)));
            rows.add(row);
        }
        var living = (net.minecraft.world.entity.LivingEntity)unsafe.allocateInstance(net.minecraft.world.entity.decoration.ArmorStand.class);
        var activeEffects = new java.util.HashMap<net.minecraft.core.Holder<net.minecraft.world.effect.MobEffect>, net.minecraft.world.effect.MobEffectInstance>();
        set(living, net.minecraft.world.entity.LivingEntity.class, "activeEffects", activeEffects);
        for (int duration = -1; duration <= 202; duration++) for (float partial : new float[]{0f, .125f, .5f, .875f, 1f}) {
            activeEffects.put(net.minecraft.world.effect.MobEffects.NIGHT_VISION, new net.minecraft.world.effect.MobEffectInstance(net.minecraft.world.effect.MobEffects.NIGHT_VISION, duration));
            var row = new JsonObject(); row.addProperty("nightDuration", duration); row.addProperty("nightPartial", partial);
            row.addProperty("nightScaleBits", Float.floatToIntBits(net.minecraft.client.renderer.GameRenderer.nightVisionScale(living, partial))); rows.add(row);
        }
        java.util.Random rateRandom = new java.util.Random(263);
        for (int i = 0; i < 500; i++) {
            float rate = (float)Math.pow(10, -5 + rateRandom.nextDouble() * 8);
            JsonObject row = new JsonObject();
            row.addProperty("rateValue", Float.toString(rate));
            row.addProperty("rateString", Float.toString(rate));
            rows.add(row);
        }
        var missingClock = commands.parse("time of example:missing query time", source);
        if (!missingClock.getReader().canRead()) throw new AssertionError("unknown clock accepted");
        set(source, CommandSourceStack.class, "permissions", PermissionSet.NO_PERMISSIONS);
        var unauthorized = commands.parse("time of example:clock query time", source);
        if (!unauthorized.getReader().canRead()) throw new AssertionError("unprivileged time command accepted");
        var weatherCommands = new CommandDispatcher<CommandSourceStack>();
        WeatherCommand.register(weatherCommands);
        set(source, CommandSourceStack.class, "permissions", PermissionSet.ALL_PERMISSIONS);
        for (String command : new String[]{"weather clear", "weather rain", "weather thunder", "weather clear 1", "weather rain .5s", "weather thunder 1d", "weather thunder 0", "weather rain -1", "weather rain 1e2", "weather snow", "weather clear 1 extra", "weather", "weather clear 0.5", "weather clear 0.4", "weather clear 2147483648", "weather clear 1t"}) {
            var parsed = weatherCommands.parse(command, source);
            JsonObject row = new JsonObject(); row.addProperty("weatherSyntax", "/" + command);
            row.addProperty("weatherSyntaxAccepted", !parsed.getReader().canRead() && parsed.getContext().getCommand() != null); rows.add(row);
        }
        set(source, CommandSourceStack.class, "permissions", PermissionSet.NO_PERMISSIONS);
        if (!weatherCommands.parse("weather clear", source).getReader().canRead()) throw new AssertionError("unprivileged weather command accepted");
        for (String json : new String[]{"{}", "{\"ignored\":42}", "[]", "null", "42", "\"text\"", "true"}) {
            JsonObject row = new JsonObject();
            row.addProperty("worldClockJson", json);
            row.addProperty("worldClockValid", WorldClock.DIRECT_CODEC.parse(JsonOps.INSTANCE, JsonParser.parseString(json)).result().isPresent());
            rows.add(row);
        }
        Files.writeString(Path.of(args[0]), rows.toString());
    }
}
