import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.options.OptionsSubScreen;
import net.minecraft.client.gui.components.OptionsList;
import net.minecraft.client.gui.components.Button;
import net.minecraft.client.gui.components.AbstractSliderButton;
import net.minecraft.client.gui.layouts.LayoutElement;
import net.minecraft.client.input.MouseButtonEvent;
import net.minecraft.client.input.MouseButtonInfo;
import net.minecraft.client.input.KeyEvent;
import net.minecraft.network.chat.Component;
import net.minecraft.sounds.SoundSource;

public class SoundOptionsOracle {
    static class TestScreen extends OptionsSubScreen {
        TestScreen(int width, int height) { super(null, null, Component.empty()); this.width=width; this.height=height; }
        protected void addOptions() {}
    }
    static class TestList extends OptionsList {
        final TestScreen screen;
        TestList(Minecraft client, TestScreen screen) { super(client, screen.width, screen); this.screen=screen; }
        void row() {
            var button = Button.builder(Component.empty(), ignored -> {}).build();
            addEntry(OptionsList.Entry.big(button, screen), 25);
        }
    }
    static class TestSlider extends AbstractSliderButton {
        TestSlider(int width) { super(58, 0, width, 20, Component.empty(), 0.5); canChangeValue=true; }
        protected void updateMessage() {}
        protected void applyValue() {}
        double getValue() { return value; }
    }
    public static void main(String[] args) throws Exception {
        for (SoundSource source:SoundSource.values()) System.out.println(source.getName());
        var field=sun.misc.Unsafe.class.getDeclaredField("theUnsafe"); field.setAccessible(true);
        var unsafe=(sun.misc.Unsafe)field.get(null);
        var client=(Minecraft)unsafe.allocateInstance(Minecraft.class);
        var instance=Minecraft.class.getDeclaredField("instance"); instance.setAccessible(true); instance.set(null,client);
        for(int width : new int[]{320,426,640}) for(int height : new int[]{240,360}) for(int scroll : new int[]{0,12,47}) {
            var screen=new TestScreen(width,height); var list=new TestList(client,screen);
            for(int row=0;row<9;row++) list.row();
            list.setScrollAmount(scroll);
            for(int category=0;category<11;category++) {
                int row=category==0?0:(category-1)/2+1;
                int column=category==0?0:(category-1)%2;
                var entry=(LayoutElement)list.children().get(row);
                System.out.println((entry.getX()+column*160)+","+(entry.getY()+2)+","+list.maxScrollAmount());
            }
        }
        for(int width:new int[]{150,310}) for(int x=45;x<=380;x++) {
            var slider=new TestSlider(width);
            slider.onClick(new MouseButtonEvent(x,0,new MouseButtonInfo(0,0)),false);
            System.out.println(Long.toUnsignedString(Double.doubleToRawLongBits(slider.getValue())));
            slider.keyPressed(new KeyEvent(80,0,0));
            System.out.println(Long.toUnsignedString(Double.doubleToRawLongBits(slider.getValue())));
            slider.keyPressed(new KeyEvent(79,0,0));
            System.out.println(Long.toUnsignedString(Double.doubleToRawLongBits(slider.getValue())));
        }
    }
}
