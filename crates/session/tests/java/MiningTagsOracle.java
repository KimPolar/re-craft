import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.core.*;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.world.item.*;
import net.minecraft.world.item.component.Tool;
import net.minecraft.world.level.block.*;
import net.minecraft.world.level.block.state.BlockState;

public class MiningTagsOracle {
    static final String[] TAGS={"mineable/pickaxe","mineable/axe","mineable/shovel","mineable/hoe","incorrect_for_wooden_tool","incorrect_for_stone_tool","incorrect_for_copper_tool","incorrect_for_iron_tool","incorrect_for_gold_tool","incorrect_for_diamond_tool","incorrect_for_netherite_tool","sword_instantly_mines","sword_efficient","shears_extreme_breaking_speed","shears_major_breaking_speed","shears_minor_breaking_speed"};
    static Tool bind(Tool source,int flags) {
        var rules=new ArrayList<Tool.Rule>();
        for(var rule:source.rules()) {
            var key=rule.blocks().unwrapKey();
            if(key.isEmpty()) { rules.add(rule); continue; }
            int bit=Arrays.asList(TAGS).indexOf(key.get().location().getPath());
            if(bit<0) throw new IllegalStateException("unexpected tool tag "+key.get());
            var blocks=(flags&(1<<bit))!=0 ? HolderSet.direct(Blocks.OAK_STAIRS.builtInRegistryHolder(),Blocks.STONE_STAIRS.builtInRegistryHolder()) : HolderSet.<Block>direct(List.of());
            rules.add(new Tool.Rule(blocks,rule.speed(),rule.correctForDrops()));
        }
        return new Tool(rules,source.defaultMiningSpeed(),source.damagePerBlock(),source.canDestroyBlocksInCreative());
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        var base=VanillaRegistries.createWorldLookup(); var reload=VanillaRegistries.createReloadableLookup(base);
        for(var p:BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(reload)) p.apply();
        var materials=new String[]{"wooden","stone","copper","iron","golden","diamond","netherite"};
        var kinds=new String[]{"sword","shovel","pickaxe","axe","hoe"};
        var originals=new ArrayList<ItemStack>();
        for(var material:materials) for(var kind:kinds) originals.add(new ItemStack(BuiltInRegistries.ITEM.getValue(Identifier.withDefaultNamespace(material+"_"+kind))));
        originals.add(new ItemStack(Items.SHEARS));
        var states=new BlockState[]{Blocks.OAK_STAIRS.defaultBlockState(),Blocks.STONE_STAIRS.defaultBlockState()};
        int seed=0x38a71c6d;
        try(var out=new DataOutputStream(Files.newOutputStream(Path.of(args[0])))) {
            for(int i=0;i<2048;i++) {
                seed=seed*1664525+1013904223;
                int flags=i<16?1<<i:i==16?0:i==17?65535:(seed>>>8)&65535;
                out.writeShort(flags);
                var tools=new ArrayList<ItemStack>();
                for(var original:originals) { var tool=original.copy(); tool.set(DataComponents.TOOL,bind(original.get(DataComponents.TOOL),flags)); tools.add(tool); }
                for(var state:states) for(var tool:tools) {
                    out.writeInt(Float.floatToRawIntBits(tool.getDestroySpeed(state)));
                    out.writeBoolean(!state.requiresCorrectToolForDrops()||tool.isCorrectToolForDrops(state));
                    out.writeByte(tool.get(DataComponents.TOOL).damagePerBlock());
                    out.writeBoolean(tool.get(DataComponents.TOOL).canDestroyBlocksInCreative());
                }
            }
        }
    }
}
