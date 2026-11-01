import com.google.gson.*;
import java.nio.file.*;
import com.mojang.blaze3d.vertex.PoseStack;
import net.minecraft.client.model.object.banner.*;
import net.minecraft.client.renderer.blockentity.BannerRenderer;
import net.minecraft.core.Direction;

public class WorldBannerGeometryOracle {
  public static void main(String[] args) throws Exception {
    net.minecraft.SharedConstants.tryDetectVersion(); net.minecraft.server.Bootstrap.bootStrap();
    var rows=new JsonArray();
    for(int index=0;index<20;index++) for(float phase:new float[]{0f,.125f,.25f,.5f,.9875f}) {
      boolean standing=index<16;
      var pose=new PoseStack();
      if(standing) pose.mulPose(BannerRenderer.TRANSFORMATIONS.freeTransformations(index));
      else pose.mulPose(BannerRenderer.TRANSFORMATIONS.wallTransformation(new Direction[]{Direction.SOUTH,Direction.WEST,Direction.NORTH,Direction.EAST}[index-16]));
      var body=BannerModel.createBodyLayer(standing).bakeRoot(); var flag=BannerFlagModel.createFlagLayer(standing).bakeRoot(); new BannerFlagModel(flag).setupAnim(phase);
      var a=new BannerGeometryOracle.Sink(); var b=new BannerGeometryOracle.Sink(); body.render(pose,a,0,0); flag.render(pose,b,0,0);
      var row=new JsonObject(); row.addProperty("index",index); row.addProperty("phase",phase); row.add("body",a.vertices); row.add("flag",b.vertices); rows.add(row);
    }
    Files.writeString(Path.of(args[0]),rows.toString());
  }
}
