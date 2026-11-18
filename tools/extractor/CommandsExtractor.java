// Extractor of the integrated server's command tree (docs/specs/commands.md § 1), run during
// preparation with the installed game's client jar. The tree is built like the server of a
// singleplayer game ("integrated" selection), then filtered for each of the five permission
// levels (0 everyone … 4 owners) as when it is sent to the player, and encoded in the exact
// format of the `commands` packet (node order, flags, suggestion ids included).
// Output: 5 × (big-endian u32 length, packet body without id), levels 0 to 4.
// Usage: java -cp <client jar;libraries> CommandsExtractor.java <output.bin>
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;
import com.mojang.brigadier.builder.ArgumentBuilder;
import com.mojang.brigadier.tree.ArgumentCommandNode;
import com.mojang.brigadier.tree.CommandNode;
import com.mojang.brigadier.tree.RootCommandNode;
import io.netty.buffer.Unpooled;
import net.minecraft.SharedConstants;
import net.minecraft.commands.CommandBuildContext;
import net.minecraft.commands.CommandSourceStack;
import net.minecraft.commands.Commands;
import net.minecraft.commands.synchronization.SuggestionProviders;
import net.minecraft.data.registries.VanillaRegistries;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.protocol.game.ClientboundCommandsPacket;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.permissions.LevelBasedPermissionSet;
import net.minecraft.server.permissions.PermissionLevel;
import net.minecraft.server.permissions.PermissionSet;

public class CommandsExtractor {
    // Copies the nodes usable by `who` under `to` (redirects followed through `done`).
    static void usable(CommandNode<CommandSourceStack> from, CommandNode<CommandSourceStack> to,
            CommandSourceStack who, Map<CommandNode<CommandSourceStack>, CommandNode<CommandSourceStack>> done) {
        for (CommandNode<CommandSourceStack> child : from.getChildren()) {
            if (!child.canUse(who)) continue;
            ArgumentBuilder<CommandSourceStack, ?> b = child.createBuilder();
            if (b.getRedirect() != null) b.redirect(done.get(b.getRedirect()));
            CommandNode<CommandSourceStack> copy = b.build();
            done.put(child, copy);
            to.addChild(copy);
            if (!child.getChildren().isEmpty()) usable(child, copy, who, done);
        }
    }

    public static void main(String[] args) throws Exception {
        if (args.length != 1) {
            System.err.println("usage: CommandsExtractor <output.bin>");
            System.exit(2);
        }
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        CommandBuildContext context = Commands.createValidationContext(VanillaRegistries.createWorldLookup());
        RootCommandNode<CommandSourceStack> all =
            new Commands(Commands.CommandSelection.INTEGRATED, context).getDispatcher().getRoot();
        CommandSourceStack nobody = Commands.createCompilationContext(PermissionSet.NO_PERMISSIONS);
        ClientboundCommandsPacket.NodeInspector<CommandSourceStack> inspector =
            new ClientboundCommandsPacket.NodeInspector<CommandSourceStack>() {
                @Override
                public Identifier suggestionId(ArgumentCommandNode<CommandSourceStack, ?> node) {
                    var p = node.getCustomSuggestions();
                    return p != null ? SuggestionProviders.getName(p) : null;
                }

                @Override
                public boolean isExecutable(CommandNode<CommandSourceStack> node) {
                    return node.getCommand() != null;
                }

                @Override
                public boolean isRestricted(CommandNode<CommandSourceStack> node) {
                    return !node.getRequirement().test(nobody);
                }
            };
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        DataOutputStream data = new DataOutputStream(out);
        for (int level = 0; level <= 4; level++) {
            CommandSourceStack who =
                Commands.createCompilationContext(LevelBasedPermissionSet.forLevel(PermissionLevel.byId(level)));
            RootCommandNode<CommandSourceStack> root = new RootCommandNode<>();
            Map<CommandNode<CommandSourceStack>, CommandNode<CommandSourceStack>> done = new HashMap<>();
            done.put(all, root);
            usable(all, root, who, done);
            FriendlyByteBuf buf = new FriendlyByteBuf(Unpooled.buffer());
            ClientboundCommandsPacket.STREAM_CODEC.encode(buf, new ClientboundCommandsPacket(root, inspector));
            byte[] body = new byte[buf.readableBytes()];
            buf.readBytes(body);
            data.writeInt(body.length);
            data.write(body);
        }
        data.flush();
        Files.write(Path.of(args[0]), out.toByteArray());
    }
}
