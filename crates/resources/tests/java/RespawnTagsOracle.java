import java.nio.file.*;
import java.util.*;
import com.google.gson.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.MinecraftServer;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.core.Holder;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.TagLoader;
import net.minecraft.world.entity.EntityTypes;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.DataPackConfig;
import net.minecraft.world.level.WorldDataConfiguration;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.CampfireBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.validation.DirectoryValidator;

public class RespawnTagsOracle {
    static void append(JsonArray rows, BlockState state, String lit) {
        JsonObject row = new JsonObject();
        row.addProperty("id", BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
        if (lit != null) row.addProperty("lit", lit);
        row.addProperty("dangerous", EntityTypes.PLAYER.isBlockDangerous(state));
        row.addProperty("climbable", state.is(BlockTags.CLIMBABLE));
        row.addProperty("invalid", state.is(BlockTags.INVALID_SPAWN_INSIDE));
        rows.add(row);
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap();
        JsonArray scenarios = new JsonArray();
        for (int scenario=0; scenario<2; scenario++) {
            var repo = ServerPacksSource.createPackRepository(Path.of(args[0]), new DirectoryValidator(path -> false));
            var selected = scenario == 0 ? List.of("vanilla") : List.of("vanilla", "file/respawn");
            var disabled = scenario == 0 ? List.of("file/respawn") : List.<String>of();
            var config = new WorldDataConfiguration(new DataPackConfig(selected, disabled), FeatureFlags.DEFAULT_FLAGS);
            MinecraftServer.configurePackRepository(repo, config, false, false);
            try (var manager = new MultiPackResourceManager(PackType.SERVER_DATA, repo.openAllSelected())) {
                TagLoader.ElementLookup<Holder<Block>> lookup = (id, required) -> BuiltInRegistries.BLOCK.get(id).map(holder -> (Holder<Block>)holder);
                var tags = TagLoader.loadTagsForRegistry(manager, Registries.BLOCK, lookup);
                BuiltInRegistries.BLOCK.prepareTagReload(new TagLoader.LoadResult<>(Registries.BLOCK, tags)).apply();
                JsonArray rows = new JsonArray();
                for (Block block : BuiltInRegistries.BLOCK) {
                    BlockState state = block.defaultBlockState();
                    if (block instanceof CampfireBlock) {
                        append(rows, state.setValue(CampfireBlock.LIT, false), "false");
                        append(rows, state.setValue(CampfireBlock.LIT, true), "true");
                    } else append(rows, state, null);
                }
                scenarios.add(rows);
            }
        }
        Files.writeString(Path.of(args[1]), new Gson().toJson(scenarios));
    }
}
