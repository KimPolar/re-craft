import com.google.gson.*;
import java.nio.file.*;
import java.lang.reflect.*;
import java.util.*;

public class SpriteAnimationOracle {
    static Field field(Class<?> type,String name) throws Exception { var f=type.getDeclaredField(name); f.setAccessible(true); return f; }
    public static void main(String[] args) throws Exception {
        net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
        var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
        Class<?> info=Class.forName("net.minecraft.client.renderer.texture.SpriteContents$FrameInfo");
        Class<?> animation=Class.forName("net.minecraft.client.renderer.texture.SpriteContents$AnimatedTexture");
        Class<?> state=Class.forName("net.minecraft.client.renderer.texture.SpriteContents$AnimationState");
        var frameCtor=info.getDeclaredConstructor(int.class,int.class); frameCtor.setAccessible(true);
        var frames=List.of(frameCtor.newInstance(2,2),frameCtor.newInstance(0,3),frameCtor.newInstance(1,2),frameCtor.newInstance(1,1));
        var tick=state.getDeclaredMethod("tick"); tick.setAccessible(true);
        var rows=new JsonArray();
        for(boolean interpolate:new boolean[]{false,true}) {
            var anim=unsafe.allocateInstance(animation); field(animation,"frames").set(anim,frames); field(animation,"interpolateFrames").setBoolean(anim,interpolate);
            var s=unsafe.allocateInstance(state); field(state,"animationInfo").set(s,anim); field(state,"isDirty").setBoolean(s,true);
            for(int time=0;time<48;time++) {
                int frame=field(state,"frame").getInt(s), sub=field(state,"subFrame").getInt(s);
                int current=field(info,"index").getInt(frames.get(frame));
                int next=field(info,"index").getInt(frames.get((frame+1)%frames.size()));
                int duration=field(info,"time").getInt(frames.get(frame));
                int progress=interpolate && current!=next ? (int)(((float)sub/(float)duration)*1000.0f) : 0;
                if(progress==0) next=current;
                var row=new JsonObject(); row.addProperty("interpolate",interpolate); row.addProperty("tick",time);
                row.addProperty("current",current); row.addProperty("next",next); row.addProperty("progress",progress); rows.add(row);
                tick.invoke(s);
            }
        }
        Files.writeString(Path.of(args[0]),new Gson().toJson(rows));
    }
}
