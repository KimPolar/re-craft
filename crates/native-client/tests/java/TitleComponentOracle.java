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
        final Map<String,String> entries=Map.of("example.order","%2$s / %1$s %%!", "example.simple","Hello %s", "example.codes","Before \u00a7a%s after");
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
            "{\"translate\":\"example.order\",\"with\":[\"only one\"]}",
            "{\"text\":\"Base \\u00a7lbold\\u00a7cRed\\u00a7rRestore\\u00a7Z?\\u00a7\",\"color\":\"gold\",\"italic\":true}",
            "{\"text\":\"A\\u00a7lBold\",\"color\":\"green\",\"extra\":[{\"text\":\" child\\u00a7NUnder\\u00a7r normal\"}]}",
            "{\"translate\":\"example.codes\",\"with\":[{\"text\":\"argument\",\"bold\":true}],\"color\":\"red\"}",
            "\"\\u00a700\\u00a711\\u00a722\\u00a733\\u00a744\\u00a755\\u00a766\\u00a777\\u00a788\\u00a799\\u00a7aa\\u00a7bb\\u00a7cc\\u00a7dd\\u00a7ee\\u00a7ff\\u00a7kX\\u00a7mY\\u00a7nZ\\u00a7oQ\\u00a7r!\"",
            "\"a\\ud800b\\u00a7\\ud83d\\ude00\""
        };
        var output=new JsonArray();
        for(int i=0;i<cases.length;i++) {
            var component=ComponentSerialization.CODEC.parse(JsonOps.INSTANCE,JsonParser.parseString(cases[i])).getOrThrow();
            var tag=ComponentSerialization.CODEC.encodeStart(NbtOps.INSTANCE,component).getOrThrow();
            var wrapper=new CompoundTag(); wrapper.put("name",tag);
            try(var stream=new DataOutputStream(Files.newOutputStream(Path.of(args[0],i+".nbt")))) { NbtIo.write(wrapper,(DataOutput)stream); }
            var chars=new JsonArray();
            net.minecraft.util.StringDecomposer.iterateFormatted(component,Style.EMPTY,(offset,style,cp)-> { var entry=new JsonObject(); entry.addProperty("cp",cp); entry.addProperty("color",style.getColor()==null ? -1 : style.getColor().getValue()); entry.addProperty("bold",style.isBold()); entry.addProperty("italic",style.isItalic()); entry.addProperty("underlined",style.isUnderlined()); entry.addProperty("strikethrough",style.isStrikethrough()); entry.addProperty("obfuscated",style.isObfuscated()); chars.add(entry); return true; });
            output.add(chars);
        }
        Files.writeString(Path.of(args[0],"expected.json"),output.toString());
    }
}
