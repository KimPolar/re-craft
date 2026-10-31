import com.google.gson.JsonParser;
import java.nio.file.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.chunk.CarvingMask;
import net.minecraft.world.level.levelgen.carver.WorldCarver;

public class CarverGeometryOracle {
  public static void main(String[] args)throws Exception {
    var out=System.out;SharedConstants.tryDetectVersion();Bootstrap.bootStrap();
    var cases=JsonParser.parseString(Files.readString(Path.of(args[0]))).getAsJsonArray();
    for(var item:cases) {
      var v=item.getAsJsonObject();var center=v.getAsJsonArray("center");
      int cx=v.get("chunk_x").getAsInt(),cz=v.get("chunk_z").getAsInt(),mode=v.get("mode").getAsInt();
      double x=center.get(0).getAsDouble(),y=center.get(1).getAsDouble(),z=center.get(2).getAsDouble();
      double horizontal=v.get("horizontal").getAsDouble(),vertical=v.get("vertical").getAsDouble();
      double floor=v.get("floor").getAsDouble();
      var chunk=new ChunkPos(cx,cz);var mask=new CarvingMask(-63,88);
      WorldCarver.carveEllipsoid(chunk,x,y,z,horizontal,vertical,mask,(xd,yd,zd,by)->switch(mode) {
        case 0 -> yd<=floor || xd*xd+yd*yd+zd*zd>=1.0;
        case 1 -> (xd*xd+zd*zd)*(1.0+0.125*(by&3))+yd*yd/6.0>=1.0;
        default -> false;
      });
      boolean reachable=WorldCarver.canReach(chunk,x,z,v.get("current").getAsInt(),v.get("total").getAsInt(),v.get("thickness").getAsFloat());
      out.printf("S %s%n",reachable);
      mask.visit((lx,lz,min,max)->out.printf("P %d %d %d %d%n",lx,lz,min,max));
    }
  }
}
