// Extractor of the creative inventory tabs (docs/specs/creative-inventory.md § 3.2), run during
// preparation with the installed game's client jar. Each tab generator is run with the base
// registries and all the features; the ordered sequence of stacks is recorded (item, components
// added in network format, visibility, required features).
// The sequences that depend on registries sent by the server (painting variants, instruments,
// enchantments) are not produced: where the generator queries one of those registries, a
// {"dyn": <registry>} marker is written and the client regenerates them.
// Banner patterns (ominous banner) are written by name for the same reason.
// Usage: java -cp <client jar;libraries> CreativeExtractor.java <output.json>
import java.lang.reflect.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import java.util.stream.Stream;
import io.netty.buffer.Unpooled;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.component.DataComponentPatch;
import net.minecraft.core.component.DataComponentType;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.component.TypedDataComponent;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.chat.contents.TranslatableContents;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.ResourceKey;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.flag.FeatureFlagSet;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.item.CreativeModeTab;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.alchemy.PotionContents;
import net.minecraft.world.level.block.entity.BannerPatternLayers;

public class CreativeExtractor {
    static StringBuilder entries;
    static boolean first;
    static RegistryAccess access;

    static String q(String s) {
        return "\"" + s.replace("\\", "\\\\").replace("\"", "\\\"") + "\"";
    }

    static void sep() {
        if (!first) entries.append(',');
        first = false;
    }

    static String features(FeatureFlagSet set) {
        StringBuilder b = new StringBuilder("[");
        List<String> names = new ArrayList<>();
        for (Identifier id : FeatureFlags.REGISTRY.toNames(set)) names.add(id.toString());
        Collections.sort(names);
        for (int i = 0; i < names.size(); i++) b.append(i > 0 ? "," : "").append(q(names.get(i)));
        return b.append(']').toString();
    }

    @SuppressWarnings("unchecked")
    static String stack(ItemStack s, int visibility) {
        StringBuilder b = new StringBuilder("{\"item\":").append(q(BuiltInRegistries.ITEM.getKey(s.getItem()).toString()));
        b.append(",\"v\":").append(visibility);
        FeatureFlagSet f = s.getItem().requiredFeatures();
        PotionContents pc = s.get(DataComponents.POTION_CONTENTS);
        if (pc != null && pc.potion().isPresent()) f = f.join(pc.potion().get().value().requiredFeatures());
        b.append(",\"f\":").append(features(f));
        StringBuilder added = new StringBuilder(), removed = new StringBuilder();
        String banner = null;
        DataComponentPatch.SplitResult patch = s.getComponentsPatch().split();
        for (DataComponentType<?> t : patch.removed()) {
            String name = BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(t).toString();
            removed.append(removed.length() > 0 ? "," : "").append(q(name));
        }
        for (TypedDataComponent<?> e : patch.added()) {
            String name = BuiltInRegistries.DATA_COMPONENT_TYPE.getKey(e.type()).toString();
            Object value = e.value();
            if (e.type() == DataComponents.BANNER_PATTERNS) {
                StringBuilder l = new StringBuilder("[");
                for (BannerPatternLayers.Layer layer : ((BannerPatternLayers) value).layers()) {
                    String p = layer.pattern().unwrapKey().orElseThrow().identifier().toString();
                    l.append(l.length() > 1 ? "," : "").append('[').append(q(p)).append(',').append(layer.color().getId()).append(']');
                }
                banner = l.append(']').toString();
                continue;
            }
            RegistryFriendlyByteBuf buf = new RegistryFriendlyByteBuf(Unpooled.buffer(), access);
            ((StreamCodec<RegistryFriendlyByteBuf, Object>) e.type().streamCodec()).encode(buf, value);
            byte[] bytes = new byte[buf.readableBytes()];
            buf.readBytes(bytes);
            added.append(added.length() > 0 ? "," : "").append('[').append(q(name)).append(',').append(q(HexFormat.of().formatHex(bytes))).append(']');
        }
        b.append(",\"c\":[").append(added).append("],\"r\":[").append(removed).append(']');
        if (banner != null) b.append(",\"banner\":").append(banner);
        return b.append('}').toString();
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        access = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        HolderLookup.Provider base = VanillaRegistries.createWorldLookup();
        // Default components of the items (normally bound when the registries load).
        BuiltInRegistries.DATA_COMPONENT_INITIALIZERS.build(base).forEach(p -> p.apply());
        Set<ResourceKey<?>> dynamic = Set.of(Registries.PAINTING_VARIANT, Registries.INSTRUMENT, Registries.ENCHANTMENT);
        HolderLookup.Provider tagged = new HolderLookup.Provider() {
            @Override
            public Stream<ResourceKey<? extends Registry<?>>> listRegistryKeys() {
                return base.listRegistryKeys();
            }

            @Override
            public <T> Optional<? extends HolderLookup.RegistryLookup<T>> lookup(ResourceKey<? extends Registry<? extends T>> key) {
                if (dynamic.contains(key)) {
                    sep();
                    entries.append("{\"dyn\":").append(q(key.identifier().toString())).append('}');
                    return Optional.empty();
                }
                return base.lookup(key);
            }
        };
        Class<?> outCls = Class.forName("net.minecraft.world.item.CreativeModeTab$Output");
        Object output = Proxy.newProxyInstance(outCls.getClassLoader(), new Class<?>[] {outCls}, (proxy, m, a) -> {
            if (m.isDefault()) return InvocationHandler.invokeDefault(proxy, m, a);
            if (m.getName().equals("accept") && a.length == 2) {
                sep();
                entries.append(stack((ItemStack) a[0], ((Enum<?>) a[1]).ordinal()));
                return null;
            }
            throw new IllegalStateException("unexpected method " + m);
        });
        Field genField = CreativeModeTab.class.getDeclaredField("displayItemsGenerator");
        genField.setAccessible(true);
        Method accept = CreativeModeTab.DisplayItemsGenerator.class.getMethod("accept", CreativeModeTab.ItemDisplayParameters.class, outCls);
        accept.setAccessible(true);
        CreativeModeTab.ItemDisplayParameters params =
                new CreativeModeTab.ItemDisplayParameters(FeatureFlags.REGISTRY.allFlags(), true, tagged);

        StringBuilder o = new StringBuilder("{\"format\":1,\"tabs\":[");
        int count = 0, stacks = 0;
        for (CreativeModeTab tab : BuiltInRegistries.CREATIVE_MODE_TAB) {
            if (count++ > 0) o.append(',');
            String title = tab.getDisplayName().getContents() instanceof TranslatableContents t ? t.getKey() : "";
            o.append("{\"name\":").append(q(BuiltInRegistries.CREATIVE_MODE_TAB.getKey(tab).toString()))
                    .append(",\"title\":").append(q(title))
                    .append(",\"row\":").append(tab.row().ordinal())
                    .append(",\"column\":").append(tab.column())
                    .append(",\"type\":").append(q(tab.getType().name().toLowerCase(Locale.ROOT)))
                    .append(",\"right\":").append(tab.isAlignedRight())
                    .append(",\"scroll\":").append(tab.canScroll())
                    .append(",\"show_title\":").append(tab.showTitle())
                    .append(",\"background\":").append(q(tab.getBackgroundTexture().toString()))
                    .append(",\"icon\":").append(stack(tab.getIconItem(), 0))
                    .append(",\"entries\":[");
            entries = new StringBuilder();
            first = true;
            if (tab.getType() == CreativeModeTab.Type.CATEGORY) accept.invoke(genField.get(tab), params, output);
            stacks += entries.length() == 0 ? 0 : entries.toString().split("\\{\"item\"", -1).length - 1;
            o.append(entries).append("]}");
        }
        // Fallback data of the dynamic registries (the server does not send their content when the
        // client announces the base pack): painting sizes, enchantment levels.
        o.append("],\"paintings\":[");
        boolean f = true;
        for (var h : base.lookupOrThrow(Registries.PAINTING_VARIANT).listElements().toList()) {
            o.append(f ? "" : ",").append('[').append(q(h.key().identifier().toString())).append(',')
                    .append(h.value().width()).append(',').append(h.value().height()).append(']');
            f = false;
        }
        o.append("],\"enchantments\":[");
        f = true;
        for (var h : base.lookupOrThrow(Registries.ENCHANTMENT).listElements().toList()) {
            o.append(f ? "" : ",").append('[').append(q(h.key().identifier().toString())).append(',')
                    .append(h.value().getMinLevel()).append(',').append(h.value().getMaxLevel()).append(']');
            f = false;
        }
        o.append("]}\n");
        Files.writeString(Path.of(args[0]), o, StandardCharsets.UTF_8);
        System.err.println(count + " creative tabs, " + stacks + " stacks → " + args[0]);
        System.exit(0);
    }
}
