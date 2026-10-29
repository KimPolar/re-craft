import java.lang.reflect.*;
import java.nio.file.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.nbt.*;
import net.minecraft.util.ProblemReporter;
import net.minecraft.world.entity.item.FallingBlockEntity;
import net.minecraft.world.level.storage.TagValueInput;
import net.minecraft.world.level.storage.ValueInput;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.SlabBlock;
import net.minecraft.world.level.block.state.properties.SlabType;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.resources.RegistryOps;
import net.minecraft.world.phys.Vec3;
import java.io.*;

public class SavedFallingOracle {
  static class Falling extends FallingBlockEntity {
    Falling() { super(null,null); }
    void loadExtra(ValueInput input) { super.readAdditionalSaveData(input); }
  }
  public static void main(String[] args) throws Exception {
    SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
    var registry=VanillaRegistries.createReloadableLookup(VanillaRegistries.createWorldLookup());
    for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(registry)) pending.apply();
    var f=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); f.setAccessible(true); var unsafe=(sun.misc.Unsafe)f.get(null);
    var cancelled=FallingBlockEntity.class.getDeclaredField("cancelDrop"); cancelled.setAccessible(true);
    Tag[] values={IntTag.valueOf(42), LongTag.valueOf(4294967297L), FloatTag.valueOf(-1.25f), DoubleTag.valueOf(8.75), StringTag.valueOf("invalid")};
    var result=new JsonArray();
    for(int i=0;i<values.length;i++) {
      var tag=new CompoundTag(); tag.putString("id","minecraft:falling_block"); tag.putIntArray("UUID",new int[]{1,2,3,4});
      var pos=new ListTag(); for(double v:new double[]{1.25,64,-2.5}) pos.add(DoubleTag.valueOf(v)); tag.put("Pos",pos);
      var motion=new ListTag(); for(double v:new double[]{0.125,-0.25,0.5}) motion.add(DoubleTag.valueOf(v)); tag.put("Motion",motion);
      var state=i==4 ? Blocks.OAK_SLAB.defaultBlockState().setValue(SlabBlock.TYPE, SlabType.TOP) : i%2==0 ? Blocks.SAND.defaultBlockState() : Blocks.RED_SAND.defaultBlockState();
      tag.put("BlockState",BlockState.CODEC.encodeStart(RegistryOps.create(NbtOps.INSTANCE,registry),state).getOrThrow());
      tag.put("Time",values[i]); tag.put("DropItem",values[i]); tag.put("CancelDrop",values[i]); tag.putBoolean("NoGravity",true); tag.putLong("unknown",17L);
      var e=(Falling)unsafe.allocateInstance(Falling.class); e.loadExtra(TagValueInput.create(ProblemReporter.DISCARDING,registry,tag));
      var row=new JsonObject(); row.addProperty("time",e.time); row.addProperty("drop",e.dropItem); row.addProperty("cancel",cancelled.getBoolean(e));
      row.addProperty("block",BuiltInRegistries.BLOCK.getKey(e.getBlockState().getBlock()).toString());
      var properties=new JsonObject(); e.getBlockState().getValues().forEach(v -> properties.addProperty(v.property().getName(),v.valueName()));
      row.add("properties",properties); result.add(row);
      NbtIo.write(tag,Path.of(args[0],"falling-"+i+".nbt"));
    }
    Files.writeString(Path.of(args[0],"expected.json"),new Gson().toJson(result));
    if(args.length > 1) {
      var in=new DataInputStream(Files.newInputStream(Path.of(args[1]))); var tag=NbtIo.read((DataInput)in); in.close();
      var input=TagValueInput.create(ProblemReporter.DISCARDING,registry,tag);
      var entity=(Falling)unsafe.allocateInstance(Falling.class); entity.loadExtra(input);
      if(entity.time != 101 || entity.dropItem || !cancelled.getBoolean(entity) || entity.getBlockState().getBlock()!=Blocks.SAND ||
         !input.read("Pos",Vec3.CODEC).orElseThrow().equals(new Vec3(-1.5,3,2.5)) ||
         !input.read("Motion",Vec3.CODEC).orElseThrow().equals(new Vec3(0.2,-0.4,0)))
        throw new AssertionError("Rust falling-block draft differs from Java decoding");
    }
    if(args.length > 2) {
      var in=new DataInputStream(Files.newInputStream(Path.of(args[2]))); var chunk=NbtIo.read((DataInput)in); in.close();
      var entities=(ListTag)chunk.get("Entities"); var falling=(CompoundTag)entities.get(0); var item=(CompoundTag)entities.get(1); var pig=(CompoundTag)entities.get(2);
      var input=TagValueInput.create(ProblemReporter.DISCARDING,registry,falling);
      var itemInput=TagValueInput.create(ProblemReporter.DISCARDING,registry,item);
      var stack=itemInput.read("Item",net.minecraft.world.item.ItemStack.CODEC).orElseThrow();
      var entity=(Falling)unsafe.allocateInstance(Falling.class); entity.loadExtra(input);
      if(entities.size()!=3 || entity.time!=101 || entity.dropItem || !cancelled.getBoolean(entity) ||
         entity.getBlockState().getBlock()!=Blocks.SAND ||
         !input.read("Pos",Vec3.CODEC).orElseThrow().equals(new Vec3(3.5,64,-2.5)) ||
         !itemInput.read("Pos",Vec3.CODEC).orElseThrow().equals(new Vec3(5.5,64,-2.5)) ||
         itemInput.getShortOr("Age",(short)0)!=12 || itemInput.getShortOr("PickupDelay",(short)0)!=9 ||
         stack.getItem()!=net.minecraft.world.item.Items.STONE || stack.getCount()!=3 ||
         !LongTag.valueOf(19L).equals(chunk.get("unknown_chunk_field")) ||
         !IntTag.valueOf(42).equals(pig.get("unknown_pig_field")) ||
         !StringTag.valueOf("minecraft:pig").equals(pig.get("id")))
        throw new AssertionError("Rust entity chunk draft differs from Java decoding");
    }
    if(args.length > 3) {
      var in=new DataInputStream(Files.newInputStream(Path.of(args[3]))); var chunk=NbtIo.read((DataInput)in); in.close();
      var position=chunk.read("Position", net.minecraft.world.level.ChunkPos.CODEC).orElseThrow();
      var entities=(ListTag)chunk.get("Entities"); var falling=(CompoundTag)entities.get(0);
      var input=TagValueInput.create(ProblemReporter.DISCARDING,registry,falling);
      var entity=(Falling)unsafe.allocateInstance(Falling.class); entity.loadExtra(input);
      if(position.x() != 1 || position.z() != -1 || chunk.getIntOr("DataVersion",0) != 5023 || entities.size()!=1 ||
         entity.time!=101 || entity.getBlockState().getBlock()!=Blocks.SAND ||
         !input.read("Pos",Vec3.CODEC).orElseThrow().equals(new Vec3(16.5,64,-2.5)))
        throw new AssertionError("Rust new entity chunk differs from Java decoding");
    }
  }
}
