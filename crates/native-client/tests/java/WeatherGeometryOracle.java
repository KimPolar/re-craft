import com.mojang.blaze3d.vertex.VertexConsumer;
import java.io.PrintWriter;
import java.lang.reflect.Field;
import java.util.List;
import java.util.Random;
import net.minecraft.client.renderer.WeatherEffectRenderer;
import net.minecraft.client.renderer.WeatherEffectRenderer.ColumnInstance;
import net.minecraft.world.phys.Vec3;
import sun.misc.Unsafe;

public class WeatherGeometryOracle {
  static class Capture implements VertexConsumer {
    float[][] xyz = new float[4][3];
    float[][] uv = new float[4][2];
    int[] color = new int[4], light = new int[4];
    int index = -1;
    public VertexConsumer addVertex(float x,float y,float z) { ++index; xyz[index]=new float[]{x,y,z}; return this; }
    public VertexConsumer setColor(int r,int g,int b,int a) { color[index]=(a<<24)|(r<<16)|(g<<8)|b; return this; }
    public VertexConsumer setColor(int value) { color[index]=value; return this; }
    public VertexConsumer setUv(float u,float v) { uv[index]=new float[]{u,v}; return this; }
    public VertexConsumer setUv1(int u,int v) { return this; }
    public VertexConsumer setUv2(int u,int v) { light[index]=u | (v << 16); return this; }
    public VertexConsumer setUv3(float u,float v) { return this; }
    public VertexConsumer setNormal(float x,float y,float z) { return this; }
    public VertexConsumer setLineWidth(float width) { return this; }
    public VertexConsumer setLight(int value) { light[index]=value; return this; }
  }
  public static void main(String[] args) throws Exception {
    Field unsafe = Unsafe.class.getDeclaredField("theUnsafe");
    unsafe.setAccessible(true);
    var renderer = (WeatherEffectRenderer)((Unsafe)unsafe.get(null)).allocateInstance(WeatherEffectRenderer.class);
    float[] sizeX = new float[1024], sizeZ = new float[1024];
    for (int z=0; z<32; ++z) for (int x=0; x<32; ++x) {
      float dx=x-16, dz=z-16, length=(float)Math.sqrt(dx*dx+dz*dz);
      sizeX[z*32+x]=-dz/length; sizeZ[z*32+x]=dx/length;
    }
    Field fx=WeatherEffectRenderer.class.getDeclaredField("columnSizeX"); fx.setAccessible(true); fx.set(renderer,sizeX);
    Field fz=WeatherEffectRenderer.class.getDeclaredField("columnSizeZ"); fz.setAccessible(true); fz.set(renderer,sizeZ);
    var method=WeatherEffectRenderer.class.getDeclaredMethod("prepareInstances", VertexConsumer.class,List.class,Vec3.class,float.class,int.class,float.class);
    method.setAccessible(true);
    var random=new Random(91826);
    try (var out=new PrintWriter(args[0])) {
      for (int i=0; i<4000; ++i) {
        int cx=random.nextInt(59999981)-29999990, cz=random.nextInt(59999981)-29999990;
        int radius=3+random.nextInt(8), dx=random.nextInt(radius*2+1)-radius, dz=random.nextInt(radius*2+1)-radius;
        if (dx==0 && dz==0) dx=1;
        int x=cx+dx,z=cz+dz, bottom=random.nextInt(384)-64,top=bottom+1+random.nextInt(20);
        double px=cx+random.nextDouble(), py=random.nextDouble()*384-64, pz=cz+random.nextDouble();
        float u=random.nextFloat()*2f-1f,v=random.nextFloat()*12f-6f,intensity=random.nextFloat();
        boolean snow=(i&1)!=0; int block=random.nextInt(16), sky=random.nextInt(16), light=(block<<4)|(sky<<20);
        var column=new ColumnInstance(x,z,bottom,top,u,v,light);
        var capture=new Capture();
        method.invoke(renderer,capture,List.of(column),new Vec3(px,py,pz),snow?0.8f:1f,radius,intensity);
        out.printf("%d %d %d %d %d %d %d %d %d %d %d %d %d %d",x,z,bottom,top,Double.doubleToRawLongBits(px),Double.doubleToRawLongBits(py),Double.doubleToRawLongBits(pz),radius,Float.floatToRawIntBits(intensity),snow?1:0,Float.floatToRawIntBits(u),Float.floatToRawIntBits(v),block,sky);
        for (int k=0;k<4;++k) out.printf(" %d %d %d %d %d %d %d",Float.floatToRawIntBits(capture.xyz[k][0]),Float.floatToRawIntBits(capture.xyz[k][1]),Float.floatToRawIntBits(capture.xyz[k][2]),Float.floatToRawIntBits(capture.uv[k][0]),Float.floatToRawIntBits(capture.uv[k][1]),capture.color[k],capture.light[k]);
        out.println();
      }
    }
  }
}
