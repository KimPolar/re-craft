import java.lang.reflect.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.core.*;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.*;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.entity.*;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.structure.BoundingBox;
import net.minecraft.world.level.levelgen.structure.structures.ShipwreckPieces;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class ShipwreckBehaviorOracle {
    public static void main(String[] args) throws Exception {
        var output=System.out;
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)uf.get(null);
        var manager=(StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        Field repository=StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        Field sizeField=StructureTemplate.class.getDeclaredField("size"); sizeField.setAccessible(true);
        Field adjusted=ShipwreckPieces.ShipwreckPiece.class.getDeclaredField("heightAdjusted"); adjusted.setAccessible(true);
        var template=new StructureTemplate();
        var id=Identifier.withDefaultNamespace("shipwreck/with_mast");
        repository.set(manager,new HashMap<>(Map.of(id,Optional.of(template))));
        for (int i=0;i<256;i++) {
            final int n=i;
            int sx=i%35, sy=(i*7)%35, sz=(i*11)%35;
            boolean beached=(i&1)!=0, already=i%7==0;
            sizeField.set(template,new Vec3i(sx,sy,sz));
            var piece=new ShipwreckPieces.ShipwreckPiece(manager,id,new BlockPos(-200+i,90,300-i*2),Rotation.values()[i%4],beached);
            adjusted.setBoolean(piece,already);
            int[] calls={0}, hash={0};
            var world=(WorldGenLevel)Proxy.newProxyInstance(WorldGenLevel.class.getClassLoader(),new Class<?>[]{WorldGenLevel.class},(p,m,v)->switch(m.getName()) {
                case "getMaxY" -> 319;
                case "getHeight" -> {
                    var map=(Heightmap.Types)v[0]; int x=(int)v[1], z=(int)v[2];
                    if (map!=(beached?Heightmap.Types.WORLD_SURFACE_WG:Heightmap.Types.OCEAN_FLOOR_WG)) throw new AssertionError("wrong heightmap");
                    calls[0]++; hash[0]=31*hash[0]+x; hash[0]=31*hash[0]+z;
                    yield -80+Math.floorMod(x*31+z*17+n*7,201);
                }
                default -> throw new AssertionError("unexpected "+m);
            });
            var random=RandomSource.create(i*987654321L-123456789L);
            var clip=new BoundingBox(-100000,-1000,-100000,100000,1000,100000);
            for (int j=0;j<2;j++) piece.postProcess(world,null,null,random,clip,new ChunkPos(0,0),BlockPos.ZERO);
            var pos=piece.templatePosition();
            output.printf("H %d %d %s %d %d %d%n",i,pos.getY(),adjusted.getBoolean(piece),calls[0],hash[0],random.nextInt());
        }
        var method=ShipwreckPieces.ShipwreckPiece.class.getDeclaredMethod("handleDataMarker",String.class,BlockPos.class,ServerLevelAccessor.class,RandomSource.class,BoundingBox.class);
        method.setAccessible(true);
        var position=new BlockPos(-7,56,21);
        var piece=new ShipwreckPieces.ShipwreckPiece(manager,id,position,Rotation.NONE,false);
        String[] markers={"map_chest","treasure_chest","supply_chest","unknown","MAP_CHEST"};
        for (int i=0;i<markers.length;i++) for (int kind=0;kind<3;kind++) {
            final int k=kind;
            var chest=new ChestBlockEntity(position.below(),Blocks.CHEST.defaultBlockState());
            var furnace=new FurnaceBlockEntity(position.below(),Blocks.FURNACE.defaultBlockState());
            int[] calls={0};
            var world=(ServerLevelAccessor)Proxy.newProxyInstance(ServerLevelAccessor.class.getClassLoader(),new Class<?>[]{ServerLevelAccessor.class},(p,m,v)-> {
                if (!m.getName().equals("getBlockEntity") || !v[0].equals(position.below())) throw new AssertionError("unexpected "+m);
                calls[0]++; return k==0?null:k==1?furnace:chest;
            });
            var random=RandomSource.create(-456789123L+i*89+kind);
            method.invoke(piece,markers[i],position,world,random,new BoundingBox(position));
            String table=chest.getLootTable()==null?"-":chest.getLootTable().identifier().toString();
            output.printf("M %d %d %d %s %d %d%n",i,kind,calls[0],table,chest.getLootTableSeed(),random.nextLong());
        }
    }
}
