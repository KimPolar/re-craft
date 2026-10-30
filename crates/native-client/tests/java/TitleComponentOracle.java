import com.google.gson.*;
import com.mojang.serialization.JsonOps;
import java.io.*;
import java.nio.file.*;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.locale.Language;
import net.minecraft.network.chat.*;
import net.minecraft.nbt.*;
import net.minecraft.util.FormattedCharSequence;

public class TitleComponentOracle {
    static class TestLanguage extends Language {
        final Map<String,String> entries=Map.of("example.order","%2$s / %1$s %%!", "example.simple","Hello %s");
        public String getOrDefault(String key,String fallback) { return entries.getOrDefault(key,fallback); }
        public boolean has(String key) { return entries.containsKey(key); }
        public boolean isDefaultRightToLeft() { return false; }
        public FormattedCharSequence getVisualOrder(FormattedText text) { return FormattedCharSequence.forward(text.getString(),Style.EMPTY); }
    }
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion(); Bootstrap.bootStrap(); Language.inject(new TestLanguage());
        String[] cases={
            "\"plain chest\"",
            "{\"text\":\"Gold\",\"color\":\"gold\",\"bold\":true,\"extra\":[{\"text\":\" Child\"},{\"text\":\" Normal\",\"bold\":false},{\"text\":\" Blue\",\"color\":\"#123456\",\"italic\":true}]}",
            "{\"translate\":\"example.order\",\"color\":\"green\",\"with\":[{\"text\":\"One\",\"color\":\"red\"},\"Two\"],\"extra\":[{\"text\":\"!\",\"underlined\":true}]}",
            "{\"translate\":\"missing.key\",\"fallback\":\"Hi %s\",\"with\":[\"guest\"],\"strikethrough\":true}",
            "{\"translate\":\"missing.key\",\"fallback\":\"bad %q\",\"with\":[\"guest\"]}",
            "[{\"text\":\"Root\",\"color\":\"red\",\"bold\":true},{\"text\":\" child\"},{\"text\":\" none\",\"bold\":false}]",
            "{\"translate\":\"example.simple\",\"with\":[{\"text\":\"Hi\",\"extra\":[{\"text\":\"!\",\"italic\":true}]}],\"color\":\"aqua\"}",
            "{\"translate\":\"example.order\",\"with\":[\"only one\"]}"
        };
        var output=new JsonArray();
        for(int i=0;i<cases.length;i++) {
            var component=ComponentSerialization.CODEC.parse(JsonOps.INSTANCE,JsonParser.parseString(cases[i])).getOrThrow();
            var tag=ComponentSerialization.CODEC.encodeStart(NbtOps.INSTANCE,component).getOrThrow();
            var wrapper=new CompoundTag(); wrapper.put("name",tag);
            try(var stream=new DataOutputStream(Files.newOutputStream(Path.of(args[0],i+".nbt")))) { NbtIo.write(wrapper,(DataOutput)stream); }
            var chars=new JsonArray();
            component.visit((style,text)-> { text.codePoints().forEach(cp -> { var entry=new JsonObject(); entry.addProperty("cp",cp); entry.addProperty("color",style.getColor()==null ? -1 : style.getColor().getValue()); entry.addProperty("bold",style.isBold()); entry.addProperty("italic",style.isItalic()); entry.addProperty("underlined",style.isUnderlined()); entry.addProperty("strikethrough",style.isStrikethrough()); entry.addProperty("obfuscated",style.isObfuscated()); chars.add(entry); }); return Optional.empty(); },Style.EMPTY);
            output.add(chars);
        }
        Files.writeString(Path.of(args[0],"expected.json"),output.toString());
    }
}
