import com.mojang.datafixers.util.Pair;
import com.mojang.serialization.Lifecycle;
import java.lang.reflect.Field;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.worldgen.Pools;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.world.level.*;
import net.minecraft.world.level.biome.*;
import net.minecraft.world.level.levelgen.*;
import net.minecraft.world.level.levelgen.structure.*;
import net.minecraft.world.level.levelgen.structure.pools.*;
import net.minecraft.world.level.levelgen.structure.pools.alias.PoolAliasLookup;
import net.minecraft.world.level.levelgen.structure.structures.JigsawStructure;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;

public class JigsawStartOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true); var unsafe=(sun.misc.Unsafe)uf.get(null);
        var manager=(StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        Field repository=StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var templates=new HashMap<Identifier,Optional<StructureTemplate>>();
        templates.put(Identifier.parse("example:a"),Optional.of(JigsawGraphOracle.template(3,false)));
        templates.put(Identifier.parse("example:b"),Optional.of(JigsawGraphOracle.template(2,false)));
        templates.put(Identifier.parse("example:end"),Optional.of(JigsawGraphOracle.template(1,true)));
        repository.set(manager,templates);
        var projection=StructureTemplatePool.Projection.RIGID;
        var a=StructurePoolElement.single("example:a").apply(projection);
        var b=StructurePoolElement.single("example:b").apply(projection);
        var end=StructurePoolElement.single("example:end").apply(projection);
        var registry=new MappedRegistry<StructureTemplatePool>(Registries.TEMPLATE_POOL,Lifecycle.stable());
        var empty=registry.register(Pools.EMPTY,new StructureTemplatePool(null,List.of()),RegistrationInfo.BUILT_IN);
        var endKey=ResourceKey.create(Registries.TEMPLATE_POOL,Identifier.parse("example:terminators"));
        var endPool=registry.register(endKey,new StructureTemplatePool(empty,List.of(Pair.of(end,1))),RegistrationInfo.BUILT_IN);
        var childKey=ResourceKey.create(Registries.TEMPLATE_POOL,Identifier.parse("example:children"));
        var children=registry.register(childKey,new StructureTemplatePool(endPool,List.of(Pair.of(a,2),Pair.of(b,1),Pair.of(EmptyPoolElement.INSTANCE,1))),RegistrationInfo.BUILT_IN);
        var missing=ResourceKey.create(Registries.TEMPLATE_POOL,Identifier.parse("example:missing"));
        registry.freeze();
        var access=new RegistryAccess.ImmutableRegistryAccess(List.of(registry));
        var generator=(JigsawPlacementOracle.FlatGenerator)unsafe.allocateInstance(JigsawPlacementOracle.FlatGenerator.class); generator.height=70;
        var biome=(Biome)unsafe.allocateInstance(Biome.class);
        var biomes=new FixedBiomeSource(Holder.direct(biome));
        var heights=LevelHeightAccessor.create(-64,384);
        for(int mode=0;mode<14;mode++) for(long seed:new long[]{0,1,2,3,42,-1,Long.MIN_VALUE,Long.MAX_VALUE}) {
            final int currentMode=mode;
            var random=new WorldgenRandom(new LegacyRandomSource(seed));
            var context=new Structure.GenerationContext(access,generator,biomes,null,biomes,null,manager,random,0,new ChunkPos(0,0),heights,h -> currentMode != 4);
            var pos=new BlockPos(-5,mode==5 || mode==6 ? -100 : 30,-8);
            var name=mode==1?Optional.of(Identifier.parse("example:door")):mode==2?Optional.of(Identifier.parse("example:missing")):Optional.<Identifier>empty();
            var project=mode==3 || mode==4?Optional.of(Heightmap.Types.WORLD_SURFACE_WG):Optional.<Heightmap.Types>empty();
            var padding=mode==6?new DimensionPadding(0):mode==7?new DimensionPadding(40,1):DimensionPadding.ZERO;
            PoolAliasLookup alias=mode==8 || mode==10?k -> endKey:mode==9?k -> missing:PoolAliasLookup.EMPTY;
            Holder<StructureTemplatePool> start=mode==11?empty:mode==10?Holder.direct(children.value()):children;
            int depth=mode>=12?mode-11:0;
            var result=JigsawPlacement.addPieces(context,start,name,depth,pos,false,project,new JigsawStructure.MaxDistance(16,12),alias,padding,LiquidSettings.APPLY_WATERLOGGING);
            if(result.isEmpty()) { out.printf("C\t%d\t%d\t-\t%d%n",mode,seed,random.nextLong()); continue; }
            var stub=result.get(); var builder=stub.getPiecesBuilder();
            if (depth==0 && !builder.isEmpty()) throw new AssertionError("depth zero must leave builder empty");
            var consumer=stub.generator().left().orElseThrow();
            var rootField=Arrays.stream(consumer.getClass().getDeclaredFields()).filter(f -> f.getType()==PoolElementStructurePiece.class).findFirst().orElseThrow();
            rootField.setAccessible(true); var piece=(PoolElementStructurePiece)rootField.get(consumer);
            var p=piece.getPosition(); var bnd=piece.getBoundingBox(); var c=stub.position();
            var id=((SinglePoolElement)piece.getElement()).getTemplateLocation();
            var graph=new StringBuilder();
            if (depth>0) for (var entry:builder.build().pieces()) {
                var q=(PoolElementStructurePiece)entry; var qp=q.getPosition(); var qb=q.getBoundingBox();
                graph.append(String.format("|%s,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d,%d",((SinglePoolElement)q.getElement()).getTemplateLocation(),q.getRotation().ordinal(),qp.getX(),qp.getY(),qp.getZ(),qb.minX(),qb.minY(),qb.minZ(),qb.maxX(),qb.maxY(),qb.maxZ(),q.getGroundLevelDelta()));
                for(var j:q.getJunctions()) graph.append(String.format(";%d,%d,%d,%d,%s",j.getSourceX(),j.getSourceGroundY(),j.getSourceZ(),j.getDeltaY(),j.getDestProjection().getSerializedName()));
            }
            out.printf("C\t%d\t%d\t%s\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d%s%n",mode,seed,id,piece.getRotation().ordinal(),p.getX(),p.getY(),p.getZ(),bnd.minX(),bnd.minY(),bnd.minZ(),bnd.maxX(),bnd.maxY(),bnd.maxZ(),c.getX(),c.getY(),c.getZ(),piece.getGroundLevelDelta(),random.nextLong(),graph);
        }
    }
}
