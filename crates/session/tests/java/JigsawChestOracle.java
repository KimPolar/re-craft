import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.ChestBlock;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.block.state.properties.ChestType;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.RotatedPillarBlock;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.FenceBlock;
import net.minecraft.world.level.block.FenceGateBlock;
import net.minecraft.world.level.block.DoorBlock;
import net.minecraft.world.level.block.WallTorchBlock;
import net.minecraft.world.level.block.IronBarsBlock;
import net.minecraft.world.level.block.StainedGlassBlock;
import net.minecraft.world.level.block.state.properties.BlockStateProperties;
import net.minecraft.world.level.lighting.LightEngine;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.block.entity.ChestBlockEntity;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.storage.TagValueInput;

public class JigsawChestOracle {
    static String mask(net.minecraft.world.phys.shapes.VoxelShape value) {
        var boxes=value.toAabbs(); var rows=new StringBuilder();
        for(int z=0;z<16;z++) {
            int bits=0;
            for(int x=0;x<16;x++) {
                double px=(x+0.5)/16.0,pz=(z+0.5)/16.0;
                for(var a:boxes) if(a.minX<px && a.maxX>px && a.minY<0.5 && a.maxY>0.5 && a.minZ<pz && a.maxZ>pz) { bits|=1<<x; break; }
            }
            if(z>0) rows.append(','); rows.append(bits);
        }
        return rows.toString();
    }
    static String shape(net.minecraft.world.phys.shapes.VoxelShape value) {
        var boxes=value.toAabbs();
        if(boxes.isEmpty()) return "-";
        if(boxes.size()!=1) throw new AssertionError("unexpected gate shape "+boxes);
        var a=boxes.get(0);
        return a.minX+","+a.minY+","+a.minZ+","+a.maxX+","+a.maxY+","+a.maxZ;
    }
    static String boxes(net.minecraft.world.phys.shapes.VoxelShape shape) {
        var result = new java.util.ArrayList<String>();
        for (var box : shape.toAabbs()) result.add(box.minX+","+box.minY+","+box.minZ+","+box.maxX+","+box.maxY+","+box.maxZ);
        return String.join(";", result);
    }
    public static void main(String[] args) throws Exception {
        var out = System.out;
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (var id : BuiltInRegistries.BLOCK.keySet()) {
            var name=id.getPath();
            if (!id.getNamespace().equals("minecraft") || !(name.equals("terracotta") || name.endsWith("_terracotta") && !name.endsWith("_glazed_terracotta"))) continue;
            var block=BuiltInRegistries.BLOCK.getValue(id);
            var state=block.defaultBlockState();
            if (block.getStateDefinition().getPossibleStates().size()!=1 || state.hasBlockEntity()
                || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                || state.getPostProcessPos(null,BlockPos.ZERO)!=null || state.useShapeForLightOcclusion()
                || !state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)
                || LightEngine.hasDifferentLightProperties(Blocks.STONE.defaultBlockState(),state)
                || !LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state))
                throw new AssertionError("unexpected terracotta effects: "+state);
            out.println("TERRACOTTA\t"+name);
        }
        for (var id : BuiltInRegistries.BLOCK.keySet()) {
            var block = BuiltInRegistries.BLOCK.getValue(id);
            if (!(block instanceof net.minecraft.world.level.block.BedBlock)) continue;
            for (var state : block.getStateDefinition().getPossibleStates()) {
                var facing=state.getValue(BlockStateProperties.HORIZONTAL_FACING);
                var part=state.getValue(BlockStateProperties.BED_PART);
                var occupied=state.getValue(BlockStateProperties.OCCUPIED);
                if (state.hasBlockEntity() || state.useShapeForLightOcclusion()
                    || state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)
                    || state.getPostProcessPos(null,BlockPos.ZERO)!=null
                    || LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state)
                    || !LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(),state))
                    throw new AssertionError("unexpected bed effects: "+state);
                out.printf("BED\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",id.getPath(),facing.getName(),part.getSerializedName(),occupied,
                    net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state),
                    boxes(state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)),boxes(state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)));
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var transformed=state.mirror(mirror).rotate(rotation);
                    out.printf("BED_TRANS\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",id.getPath(),facing.getName(),part.getSerializedName(),occupied,
                        mirror.name(),rotation.name(),transformed.getValue(BlockStateProperties.HORIZONTAL_FACING).getName(),
                        transformed.getValue(BlockStateProperties.BED_PART).getSerializedName(),transformed.getValue(BlockStateProperties.OCCUPIED));
                }
            }
        }
        for(int count=1;count<=4;count++) for(boolean lit:new boolean[]{false,true})
            for(boolean wet:new boolean[]{false,true}) for(boolean source:new boolean[]{false,true}) {
                var initial=Blocks.CANDLE.defaultBlockState().setValue(net.minecraft.world.level.block.CandleBlock.CANDLES,count)
                    .setValue(BlockStateProperties.LIT,lit).setValue(BlockStateProperties.WATERLOGGED,wet);
                var written=new java.util.ArrayList<net.minecraft.world.level.block.state.BlockState>();
                var ticks=new net.minecraft.world.ticks.ProtoChunkTicks<net.minecraft.world.level.material.Fluid>();
                var level=(net.minecraft.world.level.LevelAccessor)java.lang.reflect.Proxy.newProxyInstance(
                    net.minecraft.world.level.LevelAccessor.class.getClassLoader(),new Class<?>[]{net.minecraft.world.level.LevelAccessor.class},
                    (proxy,method,values)->switch(method.getName()) {
                        case "isClientSide" -> false;
                        case "setBlock", "setBlockAndUpdate" -> { written.add((net.minecraft.world.level.block.state.BlockState)values[1]); yield true; }
                        case "scheduleTick" -> {ticks.schedule(new net.minecraft.world.ticks.ScheduledTick<>((net.minecraft.world.level.material.Fluid)values[1],(BlockPos)values[0],(Integer)values[2],0));yield null;}
                        case "playSound", "gameEvent", "addParticle" -> null;
                        default -> throw new UnsupportedOperationException(method.toString());
                    });
                var fluid=(source?net.minecraft.world.level.material.Fluids.WATER:net.minecraft.world.level.material.Fluids.FLOWING_WATER).defaultFluidState();
                boolean changed=((net.minecraft.world.level.block.CandleBlock)Blocks.CANDLE).placeLiquid(level,BlockPos.ZERO,initial,fluid);
                var result=written.isEmpty()?initial:written.get(written.size()-1);
                if(changed!=(source && !wet) || written.size()!=(changed?1:0) || ticks.scheduledTicks().size()!=(changed?1:0))throw new AssertionError("candle waterlogging");
                if(changed && ticks.scheduledTicks().get(0).delay()!=0)throw new AssertionError("candle water tick");
                out.printf("CANDLE_LIQUID\t%d\t%s\t%s\t%s\t%s\t%s\t%s%n",count,lit,wet,source,changed,result.getValue(BlockStateProperties.LIT),result.getValue(BlockStateProperties.WATERLOGGED));
            }
        for(var block:new net.minecraft.world.level.block.Block[]{Blocks.OAK_FENCE,Blocks.OAK_SLAB,Blocks.GLASS_PANE,Blocks.IRON_BARS,Blocks.OAK_STAIRS,Blocks.DARK_OAK_TRAPDOOR}) {
            var ticks=new net.minecraft.world.ticks.ProtoChunkTicks<net.minecraft.world.level.material.Fluid>();
            var delays=new java.util.ArrayList<Integer>();
            var states=new java.util.ArrayList<net.minecraft.world.level.block.state.BlockState>();
            var flags=new java.util.ArrayList<Integer>();
            var level=(net.minecraft.world.level.LevelAccessor)java.lang.reflect.Proxy.newProxyInstance(
                net.minecraft.world.level.LevelAccessor.class.getClassLoader(),new Class<?>[]{net.minecraft.world.level.LevelAccessor.class},
                (proxy,method,values)->switch(method.getName()) {
                    case "isClientSide" -> false;
                    case "setBlock" -> { states.add((net.minecraft.world.level.block.state.BlockState)values[1]); flags.add((Integer)values[2]); yield true; }
                    case "setBlockAndUpdate" -> { states.add((net.minecraft.world.level.block.state.BlockState)values[1]); flags.add(3); yield true; }
                    case "scheduleTick" -> { int delay=(Integer)values[2]; delays.add(delay); ticks.schedule(new net.minecraft.world.ticks.ScheduledTick<>((net.minecraft.world.level.material.Fluid)values[1],(BlockPos)values[0],delay,0)); yield null; }
                    default -> throw new UnsupportedOperationException(method.toString());
                });
            var state=block.defaultBlockState();
            boolean placed=((net.minecraft.world.level.block.SimpleWaterloggedBlock)block).placeLiquid(level,BlockPos.ZERO,state,net.minecraft.world.level.material.Fluids.WATER.defaultFluidState());
            if(!placed || states.size()!=1 || !states.get(0).getValue(BlockStateProperties.WATERLOGGED) || !flags.equals(java.util.List.of(3))
                || !delays.equals(java.util.List.of(5)) || ticks.scheduledTicks().size()!=1 || ticks.scheduledTicks().get(0).delay()!=0) throw new AssertionError(block+" "+delays+" "+ticks.scheduledTicks());
        }
        out.println("PROTO_WATER_TICK\t0");
        var trapdoorUpdate=net.minecraft.world.level.block.TrapDoorBlock.class.getDeclaredMethod("updateShape",
            net.minecraft.world.level.block.state.BlockState.class,net.minecraft.world.level.LevelReader.class,
            net.minecraft.world.level.ScheduledTickAccess.class,BlockPos.class,Direction.class,BlockPos.class,
            net.minecraft.world.level.block.state.BlockState.class,net.minecraft.util.RandomSource.class);
        trapdoorUpdate.setAccessible(true);
        for(boolean wet:new boolean[]{false,true}) for(var direction:Direction.values()) {
            var delays=new java.util.ArrayList<Integer>();
            var level=(net.minecraft.world.level.LevelAccessor)java.lang.reflect.Proxy.newProxyInstance(
                net.minecraft.world.level.LevelAccessor.class.getClassLoader(),new Class<?>[]{net.minecraft.world.level.LevelAccessor.class},
                (proxy,method,values)->switch(method.getName()) {
                    case "scheduleTick" -> {delays.add((Integer)values[2]);yield null;}
                    default -> throw new AssertionError("unexpected trapdoor "+method);
                });
            var state=Blocks.DARK_OAK_TRAPDOOR.defaultBlockState().setValue(BlockStateProperties.WATERLOGGED,wet);
            var updated=(net.minecraft.world.level.block.state.BlockState)trapdoorUpdate.invoke(Blocks.DARK_OAK_TRAPDOOR,state,level,level,BlockPos.ZERO,direction,BlockPos.ZERO.relative(direction),Blocks.AIR.defaultBlockState(),net.minecraft.util.RandomSource.create(0));
            if(updated!=state || !delays.equals(wet?java.util.List.of(5):java.util.List.of())) throw new AssertionError("trapdoor update "+wet+" "+direction+" "+delays);
        }
        out.println("TRAPDOOR_WATER_UPDATE\t12");
        var lookup = VanillaRegistries.createWorldLookup();
        var registries = VanillaRegistries.createReloadableLookup(lookup);
        for (var pending : net.minecraft.core.registries.BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registries)) pending.apply();
        var chest = new ChestBlockEntity(new BlockPos(1, 8, 1), Blocks.CHEST.defaultBlockState());
        var tag = new CompoundTag();
        tag.putString("id", "minecraft:chest");
        tag.putString("LootTable", "minecraft:chests/village/village_armorer");
        tag.putLong("LootTableSeed", 123);
        var random = new WorldgenRandom(new XoroshiroRandomSource(42L));
        tag.putLong("LootTableSeed", random.nextLong());
        chest.loadWithComponents(TagValueInput.create(ProblemReporter.DISCARDING, registries, tag));
        var saved = chest.saveWithFullMetadata(registries);
        out.printf("CHEST\t%s\t%d\t%d%n", chest.getLootTable().identifier(), chest.getLootTableSeed(), random.nextLong());
        if (!saved.contains("LootTable") || !saved.contains("LootTableSeed")) throw new AssertionError(saved);
        for (var facing : new Direction[]{Direction.NORTH, Direction.EAST, Direction.SOUTH, Direction.WEST})
            for (var type : ChestType.values()) for (var wet : new boolean[]{false, true})
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var state = Blocks.CHEST.defaultBlockState().setValue(ChestBlock.FACING, facing)
                        .setValue(ChestBlock.TYPE, type).setValue(ChestBlock.WATERLOGGED, wet)
                        .mirror(mirror).rotate(rotation);
                    out.printf("STATE\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",
                        facing.getSerializedName(), type.getSerializedName(), wet, mirror.name(), rotation.name(),
                        state.getValue(ChestBlock.FACING).getSerializedName(), state.getValue(ChestBlock.TYPE).getSerializedName(), state.getValue(ChestBlock.WATERLOGGED));
                }
        for (var block : new net.minecraft.world.level.block.Block[]{Blocks.LANTERN, Blocks.SOUL_LANTERN})
            for (boolean hanging : new boolean[]{false, true}) for (boolean wet : new boolean[]{false, true}) {
                var state = block.defaultBlockState().setValue(BlockStateProperties.HANGING, hanging)
                        .setValue(BlockStateProperties.WATERLOGGED, wet);
                if (state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                        || state.getPostProcessPos(null, BlockPos.ZERO) != null
                        || state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)
                        || state.getFluidState().isSource() != wet)
                    throw new AssertionError("unexpected lantern effects: " + state);
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var transformed = state.mirror(mirror).rotate(rotation);
                    out.printf("LANTERN\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%d%n",
                        BuiltInRegistries.BLOCK.getKey(block).getPath(), hanging, wet, mirror.name(), rotation.name(),
                        transformed.getValue(BlockStateProperties.HANGING), transformed.getValue(BlockStateProperties.WATERLOGGED),
                        state.getLightEmission());
                }
            }
        for (var id : BuiltInRegistries.BLOCK.keySet()) {
            var block = BuiltInRegistries.BLOCK.getValue(id);
            if (!(block instanceof net.minecraft.world.level.block.CandleBlock)) continue;
            for (var state : block.getStateDefinition().getPossibleStates()) {
                int count=state.getValue(net.minecraft.world.level.block.CandleBlock.CANDLES);
                boolean lit=state.getValue(BlockStateProperties.LIT), wet=state.getValue(BlockStateProperties.WATERLOGGED);
                var shape=state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO).toAabbs();
                if(shape.size()!=1 || !shape.equals(state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO).toAabbs())
                    || state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                    || state.getPostProcessPos(null,BlockPos.ZERO)!=null || state.useShapeForLightOcclusion()
                    || state.getFluidState().isSource()!=wet) throw new AssertionError(state);
                var box=shape.get(0);
                if(box.minY!=0.0 || box.maxY!=0.375)throw new AssertionError("unexpected candle height");
                for(var mirror:Mirror.values())for(var rotation:Rotation.values()) {
                    if(state.mirror(mirror).rotate(rotation)!=state)throw new AssertionError("candle changed under transform");
                    out.printf("CANDLE\t%s\t%d\t%s\t%s\t%s\t%s\t%d\t%d\t%d\t%d\t%d\t%d%n",
                        id.getPath(),count,lit,wet,mirror.name(),rotation.name(),state.getLightEmission(),state.getLightDampening(),
                        Math.round(box.minX*16),Math.round(box.minZ*16),Math.round(box.maxX*16),Math.round(box.maxZ*16));
                }
            }
        }
        String[] stones = {"stone", "deepslate", "cobbled_deepslate", "polished_deepslate", "deepslate_bricks",
            "cracked_deepslate_bricks", "deepslate_tiles", "cracked_deepslate_tiles", "chiseled_deepslate",
            "reinforced_deepslate", "basalt", "polished_basalt", "smooth_basalt",
            "white_wool", "orange_wool", "magenta_wool", "light_blue_wool", "yellow_wool", "lime_wool",
            "pink_wool", "gray_wool", "light_gray_wool", "cyan_wool", "purple_wool", "blue_wool",
            "brown_wool", "green_wool", "red_wool", "black_wool"};
        for (var name : stones) {
            var state = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + name)).defaultBlockState();
            out.printf("STONE\t%s\t%s\t%s\t%d%n", name, state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO), state.hasBlockEntity(), state.getBlock().getStateDefinition().getPossibleStates().size());
        }
        for (var family : new String[]{"oak","spruce","birch","jungle","acacia","dark_oak","mangrove","cherry","pale_oak","bamboo","crimson","warped","poplar"}) {
            var names = new java.util.ArrayList<String>();
            names.add(family + "_planks");
            if (family.equals("bamboo")) {
                names.add("bamboo_block"); names.add("stripped_bamboo_block");
            } else {
                var a = family.equals("crimson") || family.equals("warped") ? "stem" : "log";
                var b = family.equals("crimson") || family.equals("warped") ? "hyphae" : "wood";
                for (var part : new String[]{a,b}) for (var prefix : new String[]{"","stripped_"}) names.add(prefix + family + "_" + part);
            }
            for (var name : names) {
                var block = BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:" + name));
                for (var original : block.getStateDefinition().getPossibleStates()) for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var rotated = original.mirror(mirror).rotate(rotation);
                    var axis = original.hasProperty(RotatedPillarBlock.AXIS) ? original.getValue(RotatedPillarBlock.AXIS).getSerializedName() : "-";
                    var nextAxis = rotated.hasProperty(RotatedPillarBlock.AXIS) ? rotated.getValue(RotatedPillarBlock.AXIS).getSerializedName() : "-";
                    out.printf("WOOD\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n", name, axis, mirror.name(), rotation.name(), nextAxis,
                        original.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO), original.hasBlockEntity(),
                        net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(original), original.getPostProcessPos(null, BlockPos.ZERO) != null);
                }
            }
        }
        for (var axis : Direction.Axis.values()) for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
            var state = Blocks.DEEPSLATE.defaultBlockState().setValue(RotatedPillarBlock.AXIS, axis).mirror(mirror).rotate(rotation);
            out.printf("AXIS\t%s\t%s\t%s\t%s%n", axis.getSerializedName(), mirror.name(), rotation.name(), state.getValue(RotatedPillarBlock.AXIS).getSerializedName());
        }
        for (var id : BuiltInRegistries.BLOCK.keySet()) {
            if (!id.getNamespace().equals("minecraft") || !id.getPath().endsWith("_slab")) continue;
            var name = id.getPath();
            var block = BuiltInRegistries.BLOCK.getValue(id);
            if (!(block instanceof SlabBlock)) throw new AssertionError(name);
            for (var type : SlabType.values()) for (var wet : new boolean[]{false, true})
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var state = block.defaultBlockState().setValue(SlabBlock.TYPE, type).setValue(SlabBlock.WATERLOGGED, wet);
                    if (state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state) || state.getPostProcessPos(null, BlockPos.ZERO) != null)
                        throw new AssertionError("unexpected slab side effect: " + name);
                    var transformed = state.mirror(mirror).rotate(rotation);
                    out.printf("SLAB\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n", name, type.getSerializedName(), wet,
                        mirror.name(), rotation.name(), state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO),
                        state.getFluidState().isSource(), transformed.getValue(SlabBlock.TYPE).getSerializedName(), transformed.getValue(SlabBlock.WATERLOGGED));
                }
        }
        for (var block : BuiltInRegistries.BLOCK) {
            if (!(block instanceof FenceGateBlock)) continue;
            var id=BuiltInRegistries.BLOCK.getKey(block);
            for(var state:block.getStateDefinition().getPossibleStates()) {
                if(state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                    || state.getPostProcessPos(null,BlockPos.ZERO)!=null || !state.getFluidState().isEmpty()
                    || state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)
                    || state.useShapeForLightOcclusion() || state.getLightDampening()!=0
                    || state.getLightEmission()!=0 || LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state)
                    || !LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(),state)) throw new AssertionError(state);
                for(var mirror:Mirror.values()) for(var rotation:Rotation.values()) {
                    var changed=state.mirror(mirror).rotate(rotation);
                    out.printf("GATE\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",id.getPath(),
                        state.getValue(BlockStateProperties.HORIZONTAL_FACING).getSerializedName(),
                        state.getValue(BlockStateProperties.IN_WALL), state.getValue(BlockStateProperties.OPEN),
                        state.getValue(BlockStateProperties.POWERED), mirror.name(), rotation.name(),
                        changed.getValue(BlockStateProperties.HORIZONTAL_FACING).getSerializedName(),
                        shape(state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)),
                        shape(state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)));
                    if(changed.getValue(BlockStateProperties.IN_WALL)!=state.getValue(BlockStateProperties.IN_WALL)
                        || changed.getValue(BlockStateProperties.OPEN)!=state.getValue(BlockStateProperties.OPEN)
                        || changed.getValue(BlockStateProperties.POWERED)!=state.getValue(BlockStateProperties.POWERED)) throw new AssertionError(changed);
                }
            }
        }
        for (var block : BuiltInRegistries.BLOCK) {
            if (!(block instanceof DoorBlock)) continue;
            var id=BuiltInRegistries.BLOCK.getKey(block);
            for(var state:block.getStateDefinition().getPossibleStates()) {
                if(state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                    || state.getPostProcessPos(null,BlockPos.ZERO)!=null || !state.getFluidState().isEmpty()
                    || state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)
                    || state.useShapeForLightOcclusion() || state.getLightDampening()!=0
                    || state.getLightEmission()!=0 || LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state)
                    || !LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(),state)) throw new AssertionError(state);
                for(var mirror:Mirror.values()) for(var rotation:Rotation.values()) {
                    var changed=state.mirror(mirror).rotate(rotation);
                    out.printf("DOOR\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",id.getPath(),
                        state.getValue(BlockStateProperties.HORIZONTAL_FACING).getSerializedName(),
                        state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF).getSerializedName(),
                        state.getValue(BlockStateProperties.DOOR_HINGE).getSerializedName(),
                        state.getValue(BlockStateProperties.OPEN), state.getValue(BlockStateProperties.POWERED),
                        mirror.name(), rotation.name(),
                        changed.getValue(BlockStateProperties.HORIZONTAL_FACING).getSerializedName(),
                        changed.getValue(BlockStateProperties.DOOR_HINGE).getSerializedName(),
                        shape(state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)),
                        shape(state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)));
                    if(changed.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF)!=state.getValue(BlockStateProperties.DOUBLE_BLOCK_HALF)
                        || changed.getValue(BlockStateProperties.OPEN)!=state.getValue(BlockStateProperties.OPEN)
                        || changed.getValue(BlockStateProperties.POWERED)!=state.getValue(BlockStateProperties.POWERED)) throw new AssertionError(changed);
                }
            }
        }
        for (var name : new String[]{"torch", "wall_torch", "soul_torch", "soul_wall_torch"}) {
            var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse("minecraft:"+name));
            for(var state:block.getStateDefinition().getPossibleStates()) {
                if(state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                    || state.getPostProcessPos(null,BlockPos.ZERO)!=null || !state.getFluidState().isEmpty()
                    || state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)
                    || state.useShapeForLightOcclusion() || state.getLightDampening()!=0
                    || state.getLightEmission()!=(name.startsWith("soul_") ? 10 : 14)
                    || !LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state)
                    || !LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(),state)) throw new AssertionError(state);
                for(var mirror:Mirror.values()) for(var rotation:Rotation.values()) {
                    var changed=state.mirror(mirror).rotate(rotation);
                    var direction=state.hasProperty(WallTorchBlock.FACING) ? state.getValue(WallTorchBlock.FACING).getSerializedName() : "-";
                    var next=changed.hasProperty(WallTorchBlock.FACING) ? changed.getValue(WallTorchBlock.FACING).getSerializedName() : "-";
                    out.printf("TORCH\t%s\t%s\t%s\t%s\t%s\t%s\t%s%n",name,direction,mirror.name(),rotation.name(),next,
                        shape(state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)),
                        shape(state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)));
                }
            }
        }
        for(var block:BuiltInRegistries.BLOCK) {
            var id=BuiltInRegistries.BLOCK.getKey(block);
            boolean pane=block instanceof IronBarsBlock;
            boolean glass=block==Blocks.GLASS || block instanceof StainedGlassBlock;
            if(!pane && !glass) continue;
            for(var state:block.getStateDefinition().getPossibleStates()) {
                boolean wet=pane && state.getValue(BlockStateProperties.WATERLOGGED);
                if(state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                    || state.getPostProcessPos(null,BlockPos.ZERO)!=null
                    || state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)!=glass
                    || state.useShapeForLightOcclusion() || state.getLightDampening()!=(wet?1:0)
                    || state.getLightEmission()!=0 || LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state)!=wet
                    || LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(),state)==wet
                    || state.getFluidState().isSource()!=wet) throw new AssertionError(state);
                for(var mirror:Mirror.values()) for(var rotation:Rotation.values()) {
                    var transformed=state.mirror(mirror).rotate(rotation);
                    int before=0,after=0;
                    if(pane) {
                        var props=new net.minecraft.world.level.block.state.properties.BooleanProperty[]{BlockStateProperties.NORTH,BlockStateProperties.EAST,BlockStateProperties.SOUTH,BlockStateProperties.WEST};
                        for(int i=0;i<4;i++) { if(state.getValue(props[i])) before|=1<<i; if(transformed.getValue(props[i])) after|=1<<i; }
                        if(transformed.getValue(BlockStateProperties.WATERLOGGED)!=wet) throw new AssertionError(transformed);
                    }
                    out.printf("GLASS\t%s\t%s\t%d\t%s\t%s\t%s\t%d\t%s\t%s%n",id.getPath(),pane,before,wet,mirror.name(),rotation.name(),after,
                        mask(state.getCollisionShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)),
                        mask(state.getShape(EmptyBlockGetter.INSTANCE,BlockPos.ZERO)));
                }
            }
        }
        for (var id : BuiltInRegistries.BLOCK.keySet()) {
            if (!id.getNamespace().equals("minecraft") || !id.getPath().endsWith("_fence")) continue;
            var block = BuiltInRegistries.BLOCK.getValue(id);
            if (!(block instanceof FenceBlock)) throw new AssertionError(id);
            for (var state : block.getStateDefinition().getPossibleStates()) {
                if (state.hasBlockEntity() || net.minecraft.world.entity.ai.village.poi.PoiTypes.hasPoi(state)
                    || state.getPostProcessPos(null, BlockPos.ZERO) != null || state.useShapeForLightOcclusion()
                    || state.isCollisionShapeFullBlock(EmptyBlockGetter.INSTANCE, BlockPos.ZERO)) throw new AssertionError("unsupported fence: "+state);
                boolean wet=state.getValue(BlockStateProperties.WATERLOGGED);
                if (LightEngine.hasDifferentLightProperties(Blocks.AIR.defaultBlockState(),state)!=wet
                    || LightEngine.hasDifferentLightProperties(Blocks.WATER.defaultBlockState(),state)==wet)
                    throw new AssertionError("unexpected fence light check: "+state);
                for (var mirror : Mirror.values()) for (var rotation : Rotation.values()) {
                    var transformed=state.mirror(mirror).rotate(rotation);
                    int before=0, after=0;
                    var props=new net.minecraft.world.level.block.state.properties.BooleanProperty[]{BlockStateProperties.NORTH,BlockStateProperties.EAST,BlockStateProperties.SOUTH,BlockStateProperties.WEST};
                    for(int i=0;i<4;i++) { if(state.getValue(props[i])) before|=1<<i; if(transformed.getValue(props[i])) after|=1<<i; }
                    out.printf("FENCE\t%s\t%d\t%s\t%s\t%s\t%d\t%s\t%s%n", id.getPath(), before, wet, mirror.name(), rotation.name(), after,
                        transformed.getValue(BlockStateProperties.WATERLOGGED),state.getFluidState().isSource());
                }
            }
        }
    }
}
