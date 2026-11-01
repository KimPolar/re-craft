import com.google.gson.*;
import java.nio.file.*;
import com.mojang.blaze3d.vertex.*;
import net.minecraft.client.model.object.banner.*;
import net.minecraft.client.model.geom.ModelPart;
import net.minecraft.world.item.DyeColor;

public class BannerGeometryOracle {
  static class Sink implements VertexConsumer {
    JsonArray vertices=new JsonArray(); JsonObject current;
    public VertexConsumer addVertex(float x,float y,float z){current=new JsonObject(); current.add("position",new Gson().toJsonTree(new float[]{x,y,z})); vertices.add(current);return this;}
    public VertexConsumer setColor(int r,int g,int b,int a){return this;} public VertexConsumer setColor(int c){return this;}
    public VertexConsumer setUv(float u,float v){current.add("uv",new Gson().toJsonTree(new float[]{u,v}));return this;}
    public VertexConsumer setUv1(int a,int b){return this;} public VertexConsumer setUv2(int a,int b){return this;} public VertexConsumer setUv3(float a,float b){return this;}
    public VertexConsumer setNormal(float x,float y,float z){current.add("normal",new Gson().toJsonTree(new float[]{x,y,z}));return this;} public VertexConsumer setLineWidth(float v){return this;}
  }
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var pose=new PoseStack(); pose.translate(.5f,0f,.5f); pose.scale(.6666667f,-.6666667f,-.6666667f);
    var body=BannerModel.createBodyLayer(true).bakeRoot(); var flag=BannerFlagModel.createFlagLayer(true).bakeRoot(); new BannerFlagModel(flag).setupAnim(0f);
    var bodySink=new Sink(); body.render(pose,bodySink,0,0); var flagSink=new Sink(); flag.render(pose,flagSink,0,0);
    var out=new JsonObject(); out.add("body",bodySink.vertices); out.add("flag",flagSink.vertices); var colors=new JsonObject(); for(var color:DyeColor.values()) colors.addProperty(color.getSerializedName(),color.getTextureDiffuseColor()); out.add("colors",colors); Files.writeString(Path.of(args[0]),out.toString());
  }
}
