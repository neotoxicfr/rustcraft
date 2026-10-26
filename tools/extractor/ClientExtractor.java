// Client-side facts extractor for Rustcraft (classes found only in the client jar).
// Run with the launcher's Java, from this source file, with the installed game's classpath.
// Writes FACTS per block state to a JSON outside the repository:
//  - faces removed against an identical block (mask of 6 directions);
//  - tint sources per layer: none, fixed colour, or biome colour (grass, foliage, dry
//    foliage, water) with the position offset that was queried;
//  - tint of block fragments (layer 0, "terrain particle" variant, same encoding);
//  - colour of the falling dust: a falling block's own colour, otherwise the terrain
//    particle tint, otherwise the map colour (same encoding, never "none").
// The game is queried with a fake level that answers with witness colours.
//
// Usage: java -cp <client jar;libraries> ClientExtractor.java <output.json>

import java.io.IOException;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

import net.minecraft.SharedConstants;
import net.minecraft.client.color.block.BlockColors;
import net.minecraft.client.color.block.BlockTintSource;
import net.minecraft.client.renderer.BiomeColors;
import net.minecraft.client.renderer.block.BlockAndTintGetter;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.CardinalLighting;
import net.minecraft.world.level.ColorResolver;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.FallingBlock;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.lighting.LevelLightEngine;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.material.Fluids;

public class ClientExtractor {
    // Tint kinds. Keep in sync with rc-data.
    static final int NONE = 0, CONSTANT = 1, GRASS = 2, FOLIAGE = 3, DRY_FOLIAGE = 4, WATER = 5, OTHER = 6;
    static final int LAYERS = 4;
    static final BlockPos PROBE = new BlockPos(100, 64, -100);
    /** Witness of the grass palette (zero alpha: never a real colour). Keep in sync with rc-data. */
    static final int GRASS_WITNESS = 0x00C0FFEE;

    /** Fake level: each biome colour resolver answers with a witness, the position is noted. */
    static final class Probe implements BlockAndTintGetter {
        int kind = NONE;
        BlockPos asked;

        @Override
        public CardinalLighting cardinalLighting() {
            return CardinalLighting.DEFAULT;
        }

        @Override
        public LevelLightEngine getLightEngine() {
            return LevelLightEngine.EMPTY;
        }

        @Override
        public int getBlockTint(BlockPos pos, ColorResolver r) {
            asked = pos;
            if (r == BiomeColors.GRASS_COLOR_RESOLVER) kind = GRASS;
            else if (r == BiomeColors.FOLIAGE_COLOR_RESOLVER) kind = FOLIAGE;
            else if (r == BiomeColors.DRY_FOLIAGE_COLOR_RESOLVER) kind = DRY_FOLIAGE;
            else if (r == BiomeColors.WATER_COLOR_RESOLVER) kind = WATER;
            else kind = OTHER;
            return 0x00ABCDEF;
        }

        @Override
        public BlockEntity getBlockEntity(BlockPos pos) {
            return null;
        }

        @Override
        public BlockState getBlockState(BlockPos pos) {
            return Blocks.AIR.defaultBlockState();
        }

        @Override
        public FluidState getFluidState(BlockPos pos) {
            return Fluids.EMPTY.defaultFluidState();
        }

        @Override
        public int getHeight() {
            return 384;
        }

        @Override
        public int getMinY() {
            return -64;
        }
    }

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: ClientExtractor <output.json>");
            System.exit(2);
        }
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        BlockColors colors = BlockColors.createDefault();

        int n = 0;
        for (BlockState ignored : Block.BLOCK_STATE_REGISTRY) n++;
        int[] skipSelf = new int[n];
        int[][] kind = new int[LAYERS][n];
        int[][] value = new int[LAYERS][n];
        int[] particleKind = new int[n], particleValue = new int[n];
        int[] dustKind = new int[n], dustValue = new int[n];
        int[] unknown = {0};
        for (BlockState s : Block.BLOCK_STATE_REGISTRY) {
            int id = Block.getId(s);
            int mask = 0;
            for (Direction d : Direction.values()) {
                if (s.skipRendering(s, d)) mask |= 1 << d.get3DDataValue();
            }
            skipSelf[id] = mask;
            for (int layer = 0; layer < LAYERS; layer++) {
                BlockTintSource src = colors.getTintSource(s, layer);
                if (src == null) continue;
                Probe p = new Probe();
                record(p, src.colorInWorld(s, p, PROBE), kind[layer], value[layer], id, unknown);
            }
            BlockTintSource first = colors.getTintSource(s, 0);
            if (first != null) {
                Probe p = new Probe();
                record(p, first.colorAsTerrainParticle(s, p, PROBE), particleKind, particleValue, id, unknown);
            }
            Probe p = new Probe();
            int dust;
            if (s.getBlock() instanceof FallingBlock falling) {
                dust = falling.getDustColor(s, p, PROBE);
            } else if (first != null) {
                dust = first.colorAsTerrainParticle(s, p, PROBE);
            } else {
                dust = s.getMapColor(p, PROBE).col;
            }
            record(p, dust, dustKind, dustValue, id, unknown);
        }
        // Colour without a position per layer (block drawn like an entity: held, displayed, in a
        // minecart). The grass palette is filled with a zero-alpha witness: the default grass
        // colour is read from the palette loaded by the client.
        int[] witness = new int[65536];
        java.util.Arrays.fill(witness, GRASS_WITNESS);
        net.minecraft.world.level.GrassColor.init(witness);
        int[][] entity = new int[LAYERS][n];
        for (BlockState s : Block.BLOCK_STATE_REGISTRY) {
            int id = Block.getId(s);
            for (int layer = 0; layer < LAYERS; layer++) {
                BlockTintSource src = colors.getTintSource(s, layer);
                entity[layer][id] = src == null ? -1 : src.color(s);
            }
        }
        try (Writer w = Files.newBufferedWriter(Path.of(args[0]), StandardCharsets.UTF_8)) {
            w.write("{\"format\":2,\"version\":\"" + SharedConstants.getCurrentVersion().name() + "\",\"state_count\":" + n);
            Extract.writeArray(w, "skip_self", skipSelf);
            for (int layer = 0; layer < LAYERS; layer++) {
                Extract.writeArray(w, "tint_kind_" + layer, kind[layer]);
                Extract.writeArray(w, "tint_value_" + layer, value[layer]);
            }
            Extract.writeArray(w, "particle_tint_kind", particleKind);
            Extract.writeArray(w, "particle_tint_value", particleValue);
            Extract.writeArray(w, "dust_kind", dustKind);
            Extract.writeArray(w, "dust_value", dustValue);
            for (int layer = 0; layer < LAYERS; layer++) {
                Extract.writeArray(w, "entity_tint_" + layer, entity[layer]);
            }
            w.write("}\n");
        }
        System.err.println(n + " states, tints with unknown modification: " + unknown[0] + " → " + args[0]);
        System.exit(0);
    }

    /** Records a colour obtained with the fake level `p`: biome witness, or fixed colour. */
    static void record(Probe p, int c, int[] kind, int[] value, int id, int[] unknown) {
        if (p.kind != NONE && c == 0x00ABCDEF) {
            kind[id] = p.kind;
            // Offset of the queried position, encoded like the seed: (d+1) per axis on 2 bits.
            int dx = p.asked.getX() - PROBE.getX(), dy = p.asked.getY() - PROBE.getY(), dz = p.asked.getZ() - PROBE.getZ();
            value[id] = (dx + 1) | (dy + 1) << 2 | (dz + 1) << 4;
        } else if (p.kind != NONE) {
            kind[id] = OTHER;
            unknown[0]++;
        } else {
            kind[id] = CONSTANT;
            value[id] = c;
        }
    }

    static final class Extract {
        static void writeArray(Writer w, String name, int[] a) throws IOException {
            StringBuilder b = new StringBuilder(a.length * 3);
            b.append(",\"").append(name).append("\":[");
            for (int i = 0; i < a.length; i++) {
                if (i > 0) b.append(',');
                b.append(a[i]);
            }
            w.write(b.append(']').toString());
        }
    }
}
