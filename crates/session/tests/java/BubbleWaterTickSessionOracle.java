import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.registries.*;
import net.minecraft.tags.*;
import net.minecraft.world.attribute.EnvironmentAttributeSystem;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.gamerules.GameRules;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.world.level.material.Fluids;
import net.minecraft.world.level.redstone.CollectingNeighborUpdater;

public class BubbleWaterTickSessionOracle {
    static final BlockPos CENTER=new BlockPos(8,10,8);
    static final Direction[] SIDES={Direction.NORTH,Direction.SOUTH,Direction.WEST,Direction.EAST};
    static class World extends IceBubbleTagOracle.World {
        GameRules rules; JsonArray changes; BlockState base;
        World() { super(); }
        @Override public BlockState getBlockState(BlockPos pos) { return blocks.getOrDefault(pos,base); }
        @Override public GameRules getGameRules() { return rules; }
        @Override public int getMinY() { return 0; }
        @Override public int getHeight() { return 32; }
        @Override public boolean setBlockAndUpdate(BlockPos pos,BlockState state) { return setBlock(pos,state,3,512); }
        @Override public boolean setBlock(BlockPos pos,BlockState state,int flags,int depth) {
            var old=getBlockState(pos);
            if(old==state) return false;
            var entry=new JsonArray(); entry.add(pos.getX()); entry.add(pos.getY()); entry.add(pos.getZ()); entry.add(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString()); entry.add(state.is(Blocks.WATER)?state.getValue(LiquidBlock.LEVEL):-1); changes.add(entry);
            return super.setBlock(pos,state,flags,depth);
        }
    }
    static BlockState cell(int code,BlockState base) { return switch(code) {
        case 0 -> base;
        case 1 -> Blocks.AIR.defaultBlockState();
        case 2 -> Blocks.WATER.defaultBlockState();
        case 3 -> Blocks.BUBBLE_COLUMN.defaultBlockState().setValue(BubbleColumnBlock.DRAG_DOWN,true);
        case 4 -> Blocks.WATER.defaultBlockState().setValue(LiquidBlock.LEVEL,1);
        default -> throw new AssertionError();
    }; }
    static List<Holder<Fluid>> fluids(int mask) {
        var fluids=new Fluid[]{Fluids.EMPTY,Fluids.WATER,Fluids.FLOWING_WATER,Fluids.LAVA,Fluids.FLOWING_LAVA};
        var result=new ArrayList<Holder<Fluid>>();
        for(int i=0;i<fluids.length;i++) if((mask & (1<<i))!=0) result.add(fluids[i].builtInRegistryHolder());
        return result;
    }
    static void fluidTags(int water,int lava,int occupiable) {
        BuiltInRegistries.FLUID.prepareTagReload(new TagLoader.LoadResult<>(Registries.FLUID,Map.of(
            FluidTags.WATER,fluids(water),FluidTags.LAVA,fluids(lava),
            FluidTags.BUBBLE_COLUMN_CAN_OCCUPY,fluids(occupiable)))).apply();
    }
    static void record(World world,JsonArray rows,BlockState base,boolean tagged,boolean convert,int waterTag,int lavaTag,int occupyTag,int below,int[] sides) {
        var solid=base.getBlock();
        world.blocks=new HashMap<>(); world.random=new LegacyRandomSource(1); world.bubble=false; world.updater=new CollectingNeighborUpdater(world,100000); world.blockTicks=new JsonArray(); world.fluidTicks=new JsonArray(); world.pending=new HashSet<>(); world.changes=new JsonArray(); world.gameEvents=0;
        world.base=base;
        var bubble=Blocks.BUBBLE_COLUMN.defaultBlockState(); world.blocks.put(CENTER,bubble);
        if(below!=0) world.blocks.put(CENTER.below(),cell(below,base));
        var sideCodes=new JsonArray();
        for(int i=0;i<4;i++) { sideCodes.add(sides[i]); if(sides[i]!=0) world.blocks.put(CENTER.relative(SIDES[i]),cell(sides[i],base)); }
        Fluids.WATER.tick(world,CENTER,bubble,bubble.getFluidState());
        var row=new JsonObject(); row.addProperty("base",BuiltInRegistries.BLOCK.getKey(solid).toString()); row.addProperty("axis",base.hasProperty(RotatedPillarBlock.AXIS)?base.getValue(RotatedPillarBlock.AXIS).getSerializedName():""); row.addProperty("tagged",tagged); row.addProperty("convert",convert); row.addProperty("waterTag",waterTag); row.addProperty("lavaTag",lavaTag); row.addProperty("occupyTag",occupyTag); row.addProperty("below",below); row.add("sides",sideCodes);
        row.add("changes",world.changes); row.add("blockTicks",world.blockTicks); row.add("fluidTicks",world.fluidTicks); row.addProperty("gameEvents",world.gameEvents); row.addProperty("next",world.random.nextInt()); rows.add(row);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        fluidTags(6,24,2);
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true); var unsafe=(sun.misc.Unsafe)field.get(null);
        var world=(World)unsafe.allocateInstance(World.class); world.attributes=EnvironmentAttributeSystem.builder().build(); world.rules=new GameRules(FeatureFlags.DEFAULT_FLAGS);
        var rows=new JsonArray();
        var solids=new Block[]{Blocks.STONE,Blocks.DIRT,Blocks.COARSE_DIRT,Blocks.COBBLESTONE,Blocks.MOSSY_COBBLESTONE,
            Blocks.GRANITE,Blocks.DIORITE,Blocks.ANDESITE,Blocks.TUFF,Blocks.SANDSTONE,Blocks.RED_SANDSTONE,
            Blocks.STONE_BRICKS,Blocks.CLAY,Blocks.MAGMA_BLOCK,Blocks.GLASS,Blocks.BEDROCK,Blocks.OBSIDIAN,
            Blocks.NETHERRACK,Blocks.END_STONE,Blocks.COBBLED_DEEPSLATE,Blocks.BLACKSTONE,Blocks.NETHER_BRICKS,
            Blocks.SOUL_SOIL,Blocks.MOSS_BLOCK};
        var bases=new ArrayList<BlockState>();
        for(var solid:solids) bases.add(solid.defaultBlockState());
        for(var solid:new Block[]{Blocks.DEEPSLATE,Blocks.BASALT}) for(var axis:Direction.Axis.values())
            bases.add(solid.defaultBlockState().setValue(RotatedPillarBlock.AXIS,axis));
        for(var base:bases) for(boolean tagged:new boolean[]{false,true}) for(boolean convert:new boolean[]{false,true}) {
            var solid=base.getBlock();
            world.rules.set(GameRules.WATER_SOURCE_CONVERSION,convert,null);
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
                BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder(),Blocks.LAVA.builtInRegistryHolder()),
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,tagged?List.of(solid.builtInRegistryHolder()):List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,List.of()))).apply();
            var mixed=solid==Blocks.STONE || solid==Blocks.MAGMA_BLOCK
                || (solid==Blocks.DEEPSLATE && base.getValue(RotatedPillarBlock.AXIS)==Direction.Axis.Y)
                || (solid==Blocks.BASALT && base.getValue(RotatedPillarBlock.AXIS)==Direction.Axis.X);
            for(int below=0;below<5;below++) {
                for(int mask=0;mask<16;mask++) for(int variant=1;variant<=4;variant++) {
                    var sideCodes=new int[4]; for(int i=0;i<4;i++) if((mask & (1<<i))!=0) sideCodes[i]=variant;
                    record(world,rows,base,tagged,convert,6,24,2,below,sideCodes);
                }
                if(mixed) for(int pattern=0;pattern<625;pattern++) {
                    var sideCodes=new int[4]; int value=pattern;
                    for(int i=0;i<4;i++) { sideCodes[i]=value%5; value/=5; }
                    record(world,rows,base,tagged,convert,6,24,2,below,sideCodes);
                }
            }
        }
        var extraBases=new BlockState[]{Blocks.STONE.defaultBlockState(),Blocks.MAGMA_BLOCK.defaultBlockState(),
            Blocks.DEEPSLATE.defaultBlockState().setValue(RotatedPillarBlock.AXIS,Direction.Axis.Y)};
        var extraSides=new int[][]{{1,1,1,1},{2,2,2,1},{3,1,4,2},{3,0,1,4}};
        var extraBelow=new int[]{0,1,4,2};
        for(var base:extraBases) for(boolean tagged:new boolean[]{false,true}) for(boolean convert:new boolean[]{false,true}) {
            var solid=base.getBlock(); world.rules.set(GameRules.WATER_SOURCE_CONVERSION,convert,null);
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
                BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder(),Blocks.LAVA.builtInRegistryHolder()),
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,tagged?List.of(solid.builtInRegistryHolder()):List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,List.of()))).apply();
            for(int water=0;water<8;water++) for(int lava=0;lava<8;lava++) {
                int waterTag=6 | ((water & 1)!=0?1:0) | ((water & 2)!=0?8:0) | ((water & 4)!=0?16:0);
                int lavaTag=((lava & 1)!=0?1:0) | ((lava & 2)!=0?8:0) | ((lava & 4)!=0?16:0);
                fluidTags(waterTag,lavaTag,2);
                for(int i=0;i<extraSides.length;i++) record(world,rows,base,tagged,convert,waterTag,lavaTag,2,extraBelow[i],extraSides[i]);
            }
        }
        for(var base:extraBases) for(boolean tagged:new boolean[]{false,true}) for(boolean convert:new boolean[]{false,true}) {
            var solid=base.getBlock(); world.rules.set(GameRules.WATER_SOURCE_CONVERSION,convert,null);
            BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK,Map.of(
                BlockTags.WASHED_AWAY_BY_FLUIDS,List.of(Blocks.AIR.builtInRegistryHolder(),Blocks.WATER.builtInRegistryHolder(),Blocks.LAVA.builtInRegistryHolder()),
                BlockTags.ENABLES_BUBBLE_COLUMN_DRAG_DOWN,tagged?List.of(solid.builtInRegistryHolder()):List.of(),
                BlockTags.ENABLES_BUBBLE_COLUMN_PUSH_UP,List.of()))).apply();
            for(int occupyTag:new int[]{0,4}) {
                fluidTags(6,24,occupyTag);
                for(int below=0;below<5;below++) for(var sideCodes:extraSides)
                    record(world,rows,base,tagged,convert,6,24,occupyTag,below,sideCodes);
            }
        }
        Files.writeString(Path.of(args[0]),rows.toString());
    }
}
