// Reference converter for the rc-datafix tests (development tool, never run by the game).
//
// Calls the official game's own data conversion on NBT files, the way the Java extractors call
// the game's API, so that our conversion can be compared with the game's on the same input.
// Run it through `oracle.py`, which builds the classpath of the installed game.
//
//   convert <type> <from-version> <in.nbt> <out.nbt>   one uncompressed named-root NBT file
//   batch <list.txt>                                   one "type from in out" line per file
//   dump <version[.sub]> <out.txt>                     every data type's shape at that version
//
// <type> is the lowercase name of one of the game's data file types (chunk, player, level,
// entity_chunk, poi_chunk, structure, hotbar, ...).

import com.mojang.datafixers.DSL;
import com.mojang.datafixers.DataFixUtils;
import com.mojang.datafixers.DataFixer;
import com.mojang.datafixers.schemas.Schema;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Locale;
import net.minecraft.SharedConstants;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.nbt.NbtIo;
import net.minecraft.util.datafix.DataFixTypes;
import net.minecraft.util.datafix.DataFixers;
import net.minecraft.util.datafix.fixes.References;

public class DatafixOracle {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        DataFixer fixer = DataFixers.getDataFixer();
        switch (args[0]) {
            case "convert" -> convert(fixer, args);
            case "batch" -> {
                for (String line : Files.readAllLines(Path.of(args[1]), StandardCharsets.UTF_8)) {
                    String[] f = line.trim().split("\\s+");
                    if (f.length >= 4) {
                        String[] withMode = new String[f.length + 1];
                        withMode[0] = "convert";
                        System.arraycopy(f, 0, withMode, 1, f.length);
                        convert(fixer, withMode);
                    }
                }
            }
            case "dump" -> dump(fixer, args[1], Path.of(args[2]));
            default -> throw new IllegalArgumentException("unknown mode " + args[0]);
        }
    }

    /** args: convert type from in out [dimension [generator]] (a chunk's conversion context). */
    private static void convert(DataFixer fixer, String[] args) throws IOException {
        String type = args[1];
        int from = Integer.parseInt(args[2]);
        Path in = Path.of(args[3]);
        Path out = Path.of(args[4]);
        CompoundTag tag;
        try (DataInputStream s = new DataInputStream(Files.newInputStream(in))) {
            tag = NbtIo.read(s, NbtAccounter.unlimitedHeap());
        }
        // A chunk is converted with its dimension and generator, given to the rules and
        // removed afterwards, the way the game's chunk reader does it.
        boolean withContext = args.length > 5;
        if (withContext) {
            CompoundTag context = new CompoundTag();
            context.putString("dimension", args[5]);
            if (args.length > 6) {
                context.putString("generator", args[6]);
            }
            tag.put("__context", context);
        }
        DataFixTypes kind = DataFixTypes.valueOf(type.toUpperCase(Locale.ROOT));
        CompoundTag fixed = kind.updateToCurrentVersion(fixer, tag, from);
        if (withContext) {
            fixed.remove("__context");
        }
        try (DataOutputStream s = new DataOutputStream(Files.newOutputStream(out))) {
            NbtIo.write(fixed, s);
        }
    }

    /** `version` is "N" or "N.sub" (a sub-version step of N). */
    private static void dump(DataFixer fixer, String version, Path out) throws Exception {
        String[] parts = version.split("\\.");
        int sub = parts.length > 1 ? Integer.parseInt(parts[1]) : 0;
        int key = DataFixUtils.makeKey(Integer.parseInt(parts[0]), sub);
        Schema schema = ((com.mojang.datafixers.DataFixerUpper) fixer).getSchema(key);
        try (PrintWriter w = new PrintWriter(Files.newBufferedWriter(out, StandardCharsets.UTF_8))) {
            for (var field : References.class.getFields()) {
                if (field.get(null) instanceof DSL.TypeReference ref) {
                    w.println("== " + ref.typeName());
                    try {
                        w.println(schema.getType(ref));
                    } catch (RuntimeException e) {
                        w.println("(absent: " + e.getMessage() + ")");
                    }
                }
            }
        }
    }
}
