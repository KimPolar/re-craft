import com.google.gson.*;
import java.nio.file.*;
import java.util.*;
import java.util.zip.ZipFile;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.*;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.Tool;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.material.Fluid;
import net.minecraft.tags.TagKey;
import net.minecraft.world.entity.EntityEquipment;
import net.minecraft.world.entity.ai.attributes.*;
import net.minecraft.world.entity.player.*;
import net.minecraft.world.effect.MobEffect;
import net.minecraft.world.level.GameType;

public class StairMiningOracle {
    static class TestPlayer extends Player {
        boolean ground,water; AttributeInstance submerged;
        TestPlayer() { super(null,null); }
        @Override public GameType gameMode() { return GameType.SURVIVAL; }
        @Override public boolean onGround() { return ground; }
        @Override public boolean isEyeInFluid(TagKey<Fluid> tag) { return water; }
        @Override public boolean hasEffect(Holder<MobEffect> effect) { return false; }
        @Override public double getAttributeValue(Holder<Attribute> attribute) { return attribute==Attributes.MINING_EFFICIENCY?0.0:1.0; }
        @Override public AttributeInstance getAttribute(Holder<Attribute> attribute) { return submerged; }
    }
    static Set<String> tag(ZipFile jar,String name,Set<String> visiting) throws Exception {
        if(!visiting.add(name)) throw new IllegalStateException("cyclic tag: "+name);
        var id=Identifier.parse(name);
        var entry=jar.getEntry("data/"+id.getNamespace()+"/tags/block/"+id.getPath()+".json");
        if(entry==null) throw new IllegalStateException("missing tag: "+name);
        var json=JsonParser.parseString(new String(jar.getInputStream(entry).readAllBytes(),java.nio.charset.StandardCharsets.UTF_8)).getAsJsonObject();
        Set<String> result=new HashSet<>();
        for(var item:json.getAsJsonArray("values")) {
            String ref=item.isJsonPrimitive()?item.getAsString():item.getAsJsonObject().get("id").getAsString();
            if(ref.startsWith("#")) result.addAll(tag(jar,ref.substring(1),visiting)); else result.add(Identifier.parse(ref).toString());
        }
        visiting.remove(name); return result;
    }
    static Tool bind(Tool source,ZipFile jar) throws Exception {
        var rules=new ArrayList<Tool.Rule>();
        for(var rule:source.rules()) {
            var key=rule.blocks().unwrapKey();
            if(key.isEmpty()) { rules.add(rule); continue; }
            var blocks=new ArrayList<Holder<Block>>();
            for(var id:tag(jar,key.get().location().toString(),new HashSet<>())) {
                var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse(id));
                if(block==null) throw new IllegalStateException("missing block "+id);
                blocks.add(block.builtInRegistryHolder());
            }
            rules.add(new Tool.Rule(HolderSet.direct(blocks),rule.speed(),rule.correctForDrops()));
        }
        return new Tool(rules,source.defaultMiningSpeed(),source.damagePerBlock(),source.canDestroyBlocksInCreative());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var base=VanillaRegistries.createWorldLookup(); var reloadable=VanillaRegistries.createReloadableLookup(base);
        for(var pending:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reloadable)) pending.apply();
        var names=new ArrayList<String>();
        for(var item:BuiltInRegistries.ITEM) if(item instanceof BlockItem block && block.getBlock() instanceof StairBlock) names.add(BuiltInRegistries.ITEM.getKey(item).toString());
        Collections.sort(names);
        var materials=new String[]{"wooden","stone","copper","iron","golden","diamond","netherite"};
        var kinds=new String[]{"sword","shovel","pickaxe","axe","hoe"};
        var unsafeField=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); unsafeField.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)unsafeField.get(null); var player=(TestPlayer)unsafe.allocateInstance(TestPlayer.class);
        var inventory=new Inventory(player,new EntityEquipment());
        var inventoryField=Player.class.getDeclaredField("inventory"); inventoryField.setAccessible(true); inventoryField.set(player,inventory);
        player.submerged=new AttributeInstance(Attributes.SUBMERGED_MINING_SPEED,value -> {}); player.submerged.setBaseValue(0.2);
        var tools=new ArrayList<ItemStack>();
        try(var jar=new ZipFile(args[1])) {
            for(var material:materials) for(var kind:kinds) {
                var item=BuiltInRegistries.ITEM.getValue(Identifier.withDefaultNamespace(material+"_"+kind));
                var stack=new ItemStack(item); stack.set(DataComponents.TOOL,bind(stack.get(DataComponents.TOOL),jar)); tools.add(stack);
            }
            var shears=new ItemStack(Items.SHEARS); shears.set(DataComponents.TOOL,bind(shears.get(DataComponents.TOOL),jar)); tools.add(shears);
            var rows=new JsonArray();
            for(var name:names) {
                var block=BuiltInRegistries.BLOCK.getValue(Identifier.parse(name)); var state=block.defaultBlockState();
                var row=new JsonObject(); row.addProperty("id",name); row.addProperty("hardness",state.getDestroySpeed(null,BlockPos.ZERO)); row.addProperty("needs_tool",state.requiresCorrectToolForDrops());
                var values=new JsonArray();
                for(var tool:tools) {
                    inventory.setItem(0,tool);
                    var v=new JsonArray(); v.add(tool.getDestroySpeed(state)); v.add(!state.requiresCorrectToolForDrops() || tool.isCorrectToolForDrops(state)); v.add(tool.get(DataComponents.TOOL).damagePerBlock()); v.add(tool.get(DataComponents.TOOL).canDestroyBlocksInCreative()); v.add(tool.getMaxDamage());
                    var progress=new JsonArray(); for(boolean ground:new boolean[]{false,true}) for(boolean water:new boolean[]{false,true}) { player.ground=ground; player.water=water; progress.add(Float.floatToRawIntBits(state.getDestroyProgress(player,null,BlockPos.ZERO))); } v.add(progress); values.add(v);
                }
                row.add("tools",values); rows.add(row);
            }
            Files.writeString(Path.of(args[0]),rows.toString());
        }
    }
}
