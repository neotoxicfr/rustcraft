// Version facts extractor for Rustcraft.
//
// Run with the launcher's Java, straight from this source file, against the player's official
// server jar (classes and libraries on the classpath). It queries the game and writes FACTS
// (collision shapes, light and physical properties per block state) to a JSON outside the
// repository. No official code or resource is copied here.
//
// Usage: java -cp <server.jar;libraries> Extractor.java <output.json>

import java.io.IOException;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.util.Mth;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.HalfTransparentBlock;
import net.minecraft.world.level.block.IceBlock;
import net.minecraft.world.level.block.LeavesBlock;
import net.minecraft.world.level.material.FlowingFluid;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.core.Direction;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.VoxelShape;

public class Extractor {
    // Bits of the per-state "flags" field. Keep in sync with rc-data.
    static final int AIR = 1, LIQUID = 1 << 1, SKYLIGHT_THROUGH = 1 << 2, CAN_OCCLUDE = 1 << 3,
            SHAPE_LIGHT_OCCLUSION = 1 << 4, SOLID_RENDER = 1 << 5, REPLACEABLE = 1 << 6,
            RANDOM_TICK = 1 << 7, LARGE_COLLISION = 1 << 8, FULL_COLLISION = 1 << 9,
            HAS_OFFSET = 1 << 10, FLUID_SOURCE = 1 << 11, LEGACY_SOLID = 1 << 12, FLUID_FALLING = 1 << 13,
            ICE = 1 << 14, FLUID_OVERLAY = 1 << 15;
    // Bits of the per-state "sim" field (integrated server). Keep in sync with rc-data (sim_bits).
    static final int CONDUCTOR = 1, SIGNAL_SOURCE = 1 << 1, BLOCK_ENTITY = 1 << 5, ANALOG = 1 << 6,
            RIGID_UP = 1 << 7, CENTER_UP = 1 << 8, CENTER_DOWN = 1 << 9, IGNITED_BY_LAVA = 1 << 10,
            PLACE_WATER = 1 << 11, LIQUID_CONTAINER = 1 << 12;

    static int sim(BlockState s, net.minecraft.world.level.BlockGetter level, BlockPos pos) {
        int v = 0;
        if (s.isRedstoneConductor(level, pos)) v |= CONDUCTOR;
        if (s.isSignalSource()) v |= SIGNAL_SOURCE;
        // Reaction to pistons: 0 normal, 1 destroyed, 2 immovable, 3 ignored, 4 pushed only.
        int push = switch (s.getPistonPushReaction()) {
            case PUSH_PULL -> 0;
            case POPPED -> 1;
            case IMMOVEABLE -> 2;
            case IGNORE_ENTITY -> 3;
            case PUSH -> 4;
        };
        v |= push << 2;
        if (s.hasBlockEntity()) v |= BLOCK_ENTITY;
        if (s.hasAnalogOutputSignal()) v |= ANALOG;
        if (s.isFaceSturdy(level, pos, Direction.UP, net.minecraft.world.level.block.SupportType.RIGID)) v |= RIGID_UP;
        if (s.isFaceSturdy(level, pos, Direction.UP, net.minecraft.world.level.block.SupportType.CENTER)) v |= CENTER_UP;
        if (s.isFaceSturdy(level, pos, Direction.DOWN, net.minecraft.world.level.block.SupportType.CENTER)) v |= CENTER_DOWN;
        if (s.ignitedByLava()) v |= IGNITED_BY_LAVA;
        if (s.getBlock() instanceof net.minecraft.world.level.block.LiquidBlockContainer c) {
            v |= LIQUID_CONTAINER;
            if (c.canPlaceLiquid(null, level, pos, s, net.minecraft.world.level.material.Fluids.WATER)) v |= PLACE_WATER;
        }
        return v;
    }

    // Table of deduplicated shapes: the empty shape has index 0.
    static final Map<String, Integer> shapeIndex = new HashMap<>();
    static final List<String> shapes = new ArrayList<>();

    static int shape(VoxelShape s) {
        StringBuilder b = new StringBuilder("[");
        boolean first = true;
        for (AABB a : s.toAabbs()) {
            if (!first) b.append(',');
            first = false;
            b.append('[').append(num(a.minX)).append(',').append(num(a.minY)).append(',').append(num(a.minZ))
                    .append(',').append(num(a.maxX)).append(',').append(num(a.maxY)).append(',').append(num(a.maxZ))
                    .append(']');
        }
        String key = b.append(']').toString();
        return shapeIndex.computeIfAbsent(key, k -> {
            shapes.add(k);
            return shapes.size() - 1;
        });
    }

    static String num(double d) {
        return d == Math.rint(d) ? Long.toString((long) d) : Double.toString(d);
    }

    /** Position whose seed gives x at its maximum (nibble 15) and y at its minimum (nibble 0): the
     *  model offset at that position reveals the block's horizontal bound and vertical bound. */
    static BlockPos offsetProbe() {
        for (int x = 0; ; x++) {
            long seed = Mth.getSeed(x, 0, 0);
            if ((seed & 15L) == 15L && (seed >> 4 & 15L) == 0L) return new BlockPos(x, 0, 0);
        }
    }

    static final BlockPos OFFSET_PROBE = offsetProbe();

    /** Random model offset: kind (0 none, 1 horizontal, 2 horizontal and vertical), effective
     *  horizontal bound, vertical bound. */
    static String offsetFields(Block b) {
        BlockState s = b.defaultBlockState();
        if (!s.hasOffsetFunction()) return ",\"offset_kind\":0";
        net.minecraft.world.phys.Vec3 o = s.getOffset(OFFSET_PROBE);
        int kind = o.y != 0.0 ? 2 : 1;
        return ",\"offset_kind\":" + kind + ",\"offset_h\":" + (float) o.x + ",\"offset_v\":" + (float) -o.y;
    }

    /** Effect of the block on an entity passing through it: 0 none, 1 cobweb, 2 berries, 3 powder snow, 4 honey, 5 bubbles,
     *  6 slime (effect when walking on it). */
    static int insideKind(Block b) {
        if (b instanceof net.minecraft.world.level.block.WebBlock) return 1;
        if (b instanceof net.minecraft.world.level.block.SweetBerryBushBlock) return 2;
        if (b instanceof net.minecraft.world.level.block.PowderSnowBlock) return 3;
        if (b instanceof net.minecraft.world.level.block.HoneyBlock) return 4;
        if (b instanceof net.minecraft.world.level.block.BubbleColumnBlock) return 5;
        if (b instanceof net.minecraft.world.level.block.SlimeBlock) return 6;
        return 0;
    }

    static String str(String s) {
        return '"' + s.replace("\\", "\\\\").replace("\"", "\\\"") + '"';
    }

    public static void main(String[] args) throws IOException {
        if (args.length != 1) {
            System.err.println("usage: Extractor <output.json>");
            System.exit(2);
        }
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        shape(net.minecraft.world.phys.shapes.Shapes.empty());

        int n = 0;
        for (BlockState ignored : Block.BLOCK_STATE_REGISTRY) n++;
        int[] collision = new int[n], outline = new int[n], occlusion = new int[n], support = new int[n];
        int[] emission = new int[n], dampening = new int[n], flags = new int[n], fluid = new int[n], fluidAmount = new int[n];
        int[] seedOffset = new int[n];
        int[] sturdy = new int[n];
        int[] lightx = new int[n];
        int[] soundType = new int[n];
        // Maps: colour of each state (index of the map colour).
        int[] mapColor = new int[n];
        // Mining: hardness (−1 unbreakable) and "requires the right tool for its drops".
        float[] hardness = new float[n];
        float[] blast = new float[n];
        int[] needsTool = new int[n];
        int[] simBits = new int[n];
        int[] instrument = new int[n];
        List<String> soundTypes = new ArrayList<>();
        Map<String, Integer> soundIndex = new HashMap<>();
        boolean[] seen = new boolean[n];
        List<String> fluids = new ArrayList<>();
        Map<String, Integer> fluidIndex = new HashMap<>();

        var level = EmptyBlockGetter.INSTANCE;
        var pos = BlockPos.ZERO;
        for (BlockState s : Block.BLOCK_STATE_REGISTRY) {
            int id = Block.getId(s);
            if (id < 0 || id >= n || seen[id]) throw new IllegalStateException("non-contiguous state ids: " + id);
            seen[id] = true;
            collision[id] = shape(s.getCollisionShape(level, pos));
            outline[id] = shape(s.getShape(level, pos));
            occlusion[id] = shape(s.getOcclusionShape());
            support[id] = shape(s.getBlockSupportShape(level, pos));
            emission[id] = s.getLightEmission();
            dampening[id] = s.getLightDampening();
            int f = 0;
            if (s.isAir()) f |= AIR;
            if (s.liquid()) f |= LIQUID;
            if (s.propagatesSkylightDown()) f |= SKYLIGHT_THROUGH;
            if (s.canOcclude()) f |= CAN_OCCLUDE;
            if (s.useShapeForLightOcclusion()) f |= SHAPE_LIGHT_OCCLUSION;
            if (s.isSolidRender()) f |= SOLID_RENDER;
            if (s.canBeReplaced()) f |= REPLACEABLE;
            if (s.isRandomlyTicking()) f |= RANDOM_TICK;
            if (s.hasLargeCollisionShape()) f |= LARGE_COLLISION;
            if (s.isCollisionShapeFullBlock(level, pos)) f |= FULL_COLLISION;
            if (s.hasOffsetFunction()) f |= HAS_OFFSET;
            FluidState fs = s.getFluidState();
            String fname = BuiltInRegistries.FLUID.getKey(fs.getType()).toString();
            fluid[id] = fluidIndex.computeIfAbsent(fname, k -> {
                fluids.add(k);
                return fluids.size() - 1;
            });
            fluidAmount[id] = fs.getAmount();
            if (!fs.isEmpty() && fs.isSource()) f |= FLUID_SOURCE;
            if (s.isSolid()) f |= LEGACY_SOLID;
            if (fs.hasProperty(FlowingFluid.FALLING) && fs.getValue(FlowingFluid.FALLING)) f |= FLUID_FALLING;
            if (s.getBlock() instanceof IceBlock) f |= ICE;
            if (s.getBlock() instanceof HalfTransparentBlock || s.getBlock() instanceof LeavesBlock) f |= FLUID_OVERLAY;
            int st = 0;
            for (Direction d : Direction.values()) {
                if (s.isFaceSturdy(level, pos, d)) st |= 1 << d.get3DDataValue();
            }
            sturdy[id] = st;
            // Lighting at meshing: emissive rendering (bit 0), dark occlusion shade (bit 1).
            lightx[id] = (s.emissiveRendering() ? 1 : 0) | (s.getShadeBrightness(level, pos) < 1.0F ? 2 : 0);
            flags[id] = f;
            seedOffset[id] = seedOffset(s);
            hardness[id] = s.getDestroySpeed(level, pos);
            blast[id] = s.getBlock().getExplosionResistance();
            needsTool[id] = s.requiresCorrectToolForDrops() ? 1 : 0;
            simBits[id] = sim(s, level, pos);
            instrument[id] = s.instrument().ordinal();
            mapColor[id] = s.getMapColor(level, pos).id;
            // The state's sound set (volume, pitch, break, step, place, hit, fall), deduplicated.
            net.minecraft.world.level.block.SoundType st2 = s.getSoundType();
            String sk = "[" + st2.getVolume() + "," + st2.getPitch() + "," + str(st2.getBreakSound().location().toString())
                    + "," + str(st2.getStepSound().location().toString()) + "," + str(st2.getPlaceSound().location().toString())
                    + "," + str(st2.getHitSound().location().toString()) + "," + str(st2.getFallSound().location().toString()) + "]";
            soundType[id] = soundIndex.computeIfAbsent(sk, k -> {
                soundTypes.add(k);
                return soundTypes.size() - 1;
            });
        }

        try (Writer w = Files.newBufferedWriter(Path.of(args[0]), StandardCharsets.UTF_8)) {
            w.write("{\"format\":1,\"version\":" + str(SharedConstants.getCurrentVersion().name()));
            w.write(",\"state_count\":" + n);
            w.write(",\"shapes\":[" + String.join(",", shapes) + "]");
            w.write(",\"fluids\":[");
            for (int i = 0; i < fluids.size(); i++) w.write((i > 0 ? "," : "") + str(fluids.get(i)));
            w.write("],\"blocks\":[");
            boolean firstBlock = true;
            for (Block b : BuiltInRegistries.BLOCK) {
                int lo = Integer.MAX_VALUE, hi = Integer.MIN_VALUE;
                for (BlockState s : b.getStateDefinition().getPossibleStates()) {
                    int id = Block.getId(s);
                    lo = Math.min(lo, id);
                    hi = Math.max(hi, id);
                }
                if (!firstBlock) w.write(',');
                firstBlock = false;
                w.write("{\"name\":" + str(BuiltInRegistries.BLOCK.getKey(b).toString()) + ",\"first\":" + lo
                        + ",\"count\":" + (hi - lo + 1) + ",\"default\":" + Block.getId(b.defaultBlockState())
                        + ",\"friction\":" + b.getFriction() + ",\"speed\":" + b.getSpeedFactor() + ",\"jump\":"
                        + b.getJumpFactor() + ",\"bounce\":" + b.getBounceRestitution() + ",\"inside\":" + insideKind(b)
                        + offsetFields(b)
                        + ",\"dynamic_shape\":" + b.hasDynamicShape() + "}");
            }
            w.write("]");
            writeArray(w, "collision", collision);
            writeArray(w, "outline", outline);
            writeArray(w, "occlusion", occlusion);
            writeArray(w, "support", support);
            writeArray(w, "light_emission", emission);
            writeArray(w, "light_dampening", dampening);
            writeArray(w, "flags", flags);
            writeArray(w, "fluid", fluid);
            writeArray(w, "fluid_amount", fluidAmount);
            writeArray(w, "seed_offset", seedOffset);
            writeArray(w, "sturdy", sturdy);
            writeArray(w, "lightx", lightx);
            writeArray(w, "sound_type", soundType);
            writeArray(w, "map_color", mapColor);
            writeFloats(w, "hardness", hardness);
            writeFloats(w, "blast", blast);
            writeArray(w, "needs_tool", needsTool);
            writeArray(w, "sim", simBits);
            writeArray(w, "instrument", instrument);
            w.write(",\"sound_types\":[" + String.join(",", soundTypes) + "]");
            // Potions: base effects (effect, amplifier, visible); colour of each effect.
            w.write(",\"potions\":{");
            boolean firstPotion = true;
            for (net.minecraft.world.item.alchemy.Potion p : BuiltInRegistries.POTION) {
                if (!firstPotion) w.write(',');
                firstPotion = false;
                w.write(str(BuiltInRegistries.POTION.getKey(p).toString()) + ":[");
                boolean firstEffect = true;
                for (net.minecraft.world.effect.MobEffectInstance e : p.getEffects()) {
                    if (!firstEffect) w.write(',');
                    firstEffect = false;
                    w.write("[" + str(BuiltInRegistries.MOB_EFFECT.getKey(e.getEffect().value()).toString()) + ","
                            + e.getAmplifier() + "," + e.isVisible() + "," + e.getDuration() + "]");
                }
                w.write("]");
            }
            // Category of each effect (0 beneficial, 1 harmful, 2 neutral): tooltip colour.
            w.write("},\"effect_categories\":{");
            boolean firstCat = true;
            for (net.minecraft.world.effect.MobEffect e : BuiltInRegistries.MOB_EFFECT) {
                if (!firstCat) w.write(',');
                firstCat = false;
                w.write(str(BuiltInRegistries.MOB_EFFECT.getKey(e).toString()) + ":" + e.getCategory().ordinal());
            }
            w.write("},\"effect_colors\":{");
            boolean firstColor = true;
            for (net.minecraft.world.effect.MobEffect e : BuiltInRegistries.MOB_EFFECT) {
                if (!firstColor) w.write(',');
                firstColor = false;
                w.write(str(BuiltInRegistries.MOB_EFFECT.getKey(e).toString()) + ":" + e.getColor());
            }
            // Dimensions per entity type: width, height, eyes, attachment point of the name.
            w.write("},\"entity_dims\":{");
            boolean firstType = true;
            for (net.minecraft.world.entity.EntityType<?> t : BuiltInRegistries.ENTITY_TYPE) {
                if (!firstType) w.write(',');
                firstType = false;
                net.minecraft.world.entity.EntityDimensions dims = t.getDimensions();
                double tag = dims.attachments().get(net.minecraft.world.entity.EntityAttachment.NAME_TAG, 0, 0.0F).y;
                w.write(str(BuiltInRegistries.ENTITY_TYPE.getKey(t).toString()) + ":[" + dims.width() + "," + dims.height() + "," + dims.eyeHeight() + "," + (float) tag + "]");
            }
            w.write("}}\n");
        }
        System.err.println(n + " states, " + shapes.size() + " distinct shapes → " + args[0]);
        System.exit(0);
    }

    // Position offset used by the state for the seed that picks model variants
    // (upper half → lower half, etc.), measured on several trial positions.
    // Encoded (dx+1) | (dy+1) << 2 | (dz+1) << 4; 21 = no offset.
    static final int[][] PROBES = {{0, 0, 0}, {1000, 64, -1000}, {17, -5, 33}, {-123, 200, 7}};

    static int seedOffset(BlockState s) {
        for (int dy = -1; dy <= 1; dy++) {
            for (int dx = -1; dx <= 1; dx++) {
                for (int dz = -1; dz <= 1; dz++) {
                    boolean all = true;
                    for (int[] p : PROBES) {
                        long got = s.getSeed(new BlockPos(p[0], p[1], p[2]));
                        if (got != Mth.getSeed(p[0] + dx, p[1] + dy, p[2] + dz)) {
                            all = false;
                            break;
                        }
                    }
                    if (all) return (dx + 1) | (dy + 1) << 2 | (dz + 1) << 4;
                }
            }
        }
        throw new IllegalStateException("seed not reproducible for " + s);
    }

    static void writeFloats(Writer w, String name, float[] a) throws IOException {
        StringBuilder b = new StringBuilder(a.length * 4);
        b.append(",\"").append(name).append("\":[");
        for (int i = 0; i < a.length; i++) {
            if (i > 0) b.append(',');
            b.append(a[i]);
        }
        w.write(b.append(']').toString());
    }

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
