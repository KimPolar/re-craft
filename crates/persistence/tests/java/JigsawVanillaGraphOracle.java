import com.mojang.serialization.Lifecycle;
import java.lang.reflect.Field;
import java.util.*;
import java.util.jar.JarFile;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.*;
import net.minecraft.resources.*;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.structure.PoolElementStructurePiece;
import net.minecraft.world.level.levelgen.structure.pools.*;
import net.minecraft.world.level.levelgen.structure.pools.alias.PoolAliasLookup;
import net.minecraft.world.level.levelgen.structure.templatesystem.*;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.*;

public class JigsawVanillaGraphOracle {
    public static void main(String[] args) throws Exception {
        var out=System.out; SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        Field uf=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); uf.setAccessible(true); var unsafe=(sun.misc.Unsafe)uf.get(null);
        var manager=(StructureTemplateManager)unsafe.allocateInstance(StructureTemplateManager.class);
        Field repository=StructureTemplateManager.class.getDeclaredField("structureRepository"); repository.setAccessible(true);
        var templates=new HashMap<Identifier,Optional<StructureTemplate>>();
        try (var jar=new JarFile(args[0])) {
            var entries=jar.entries();
            while(entries.hasMoreElements()) {
                var entry=entries.nextElement(); String name=entry.getName();
                String prefix="data/minecraft/structure/ancient_city/";
                if(!name.startsWith(prefix) || !name.endsWith(".nbt")) continue;
                var id=Identifier.parse("minecraft:"+name.substring("data/minecraft/structure/".length(),name.length()-4));
                var template=new StructureTemplate();
                try(var stream=jar.getInputStream(entry)) { template.load(BuiltInRegistries.BLOCK,NbtIo.readCompressed(stream,NbtAccounter.unlimitedHeap())); }
                templates.put(id,Optional.of(template));
            }
        }
        repository.set(manager,templates);
        var lookup=VanillaRegistries.createWorldLookup().lookupOrThrow(Registries.TEMPLATE_POOL);
        var registry=new MappedRegistry<StructureTemplatePool>(Registries.TEMPLATE_POOL,Lifecycle.stable());
        lookup.listElements().forEach(holder -> registry.register(holder.key(),holder.value(),RegistrationInfo.BUILT_IN));
        registry.freeze();
        var key=ResourceKey.create(Registries.TEMPLATE_POOL,Identifier.parse("minecraft:ancient_city/city_center"));
        var rootElement=registry.getOrThrow(key).value().getTemplates().get(0).getFirst();
        var generator=(JigsawPlacementOracle.FlatGenerator)unsafe.allocateInstance(JigsawPlacementOracle.FlatGenerator.class); generator.height=-27;
        var method=Arrays.stream(JigsawPlacement.class.getDeclaredMethods()).filter(m -> m.getName().equals("addPieces") && m.getParameterCount()==13).findFirst().orElseThrow(); method.setAccessible(true);
        for(long seed:new long[]{0,42,-1}) {
            var pos=new BlockPos(0,-27,0); var bounds=rootElement.getBoundingBox(manager,pos,Rotation.NONE);
            var root=new PoolElementStructurePiece(manager,rootElement,pos,1,Rotation.NONE,bounds,LiquidSettings.APPLY_WATERLOGGING);
            var pieces=new ArrayList<PoolElementStructurePiece>(); pieces.add(root);
            var random=new LegacyRandomSource(seed);
            var free=Shapes.join(Shapes.create(new AABB(-128,-64,-128,129,101,129)),Shapes.create(AABB.of(bounds)),BooleanOp.ONLY_FIRST);
            method.invoke(null,null,2,false,generator,manager,null,random,registry,root,pieces,free,PoolAliasLookup.EMPTY,LiquidSettings.APPLY_WATERLOGGING);
            out.printf("C\t%d\t%d\t%d%n",seed,pieces.size(),random.nextLong());
            for(var piece:pieces) {
                var p=piece.getPosition(); var b=piece.getBoundingBox();
                out.printf("P\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d%n",piece.getRotation().ordinal(),p.getX(),p.getY(),p.getZ(),b.minX(),b.minY(),b.minZ(),b.maxX(),b.maxY(),b.maxZ(),piece.getGroundLevelDelta(),piece.getJunctions().size());
                for(var j:piece.getJunctions()) out.printf("J\t%d\t%d\t%d\t%d\t%s%n",j.getSourceX(),j.getSourceGroundY(),j.getSourceZ(),j.getDeltaY(),j.getDestProjection().getSerializedName());
            }
        }
    }
}
