// Extractor of the entity models (run during preparation, with the installed game's client jar):
// all the entity model layers → JSON (docs/specs/entities.md part 2 § 2), with no window or
// GPU. The private fields of the definitions are read by reflection.
// Usage: java -cp <client jar;libraries> ModelExtractor.java <output.json> [<animations.json>]
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.*;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.client.model.geom.*;
import net.minecraft.client.model.geom.builders.*;
import net.minecraft.core.Direction;

public class ModelExtractor {
    static Field field(Class<?> c, String n) throws Exception {
        Field x = c.getDeclaredField(n);
        x.setAccessible(true);
        return x;
    }

    static String num(float v) {
        return v == (long) v ? Long.toString((long) v) : Float.toString(v);
    }

    static final StringBuilder out = new StringBuilder();
    static Field fCubes, fPose, cdTex, cdOrigin, cdDim, cdGrow, cdMirror, cdScale, cdFaces, gx, gy, gz, mpChildren;

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Map<ModelLayerLocation, LayerDefinition> roots = LayerDefinitions.createRoots();
        Field fMesh = field(LayerDefinition.class, "mesh"), fMat = field(LayerDefinition.class, "material");
        Field fTw = field(MaterialDefinition.class, "xTexSize"), fTh = field(MaterialDefinition.class, "yTexSize");
        fCubes = field(PartDefinition.class, "cubes");
        fPose = field(PartDefinition.class, "partPose");
        cdTex = field(CubeDefinition.class, "texCoord");
        cdOrigin = field(CubeDefinition.class, "origin");
        cdDim = field(CubeDefinition.class, "dimensions");
        cdGrow = field(CubeDefinition.class, "grow");
        cdMirror = field(CubeDefinition.class, "mirror");
        cdScale = field(CubeDefinition.class, "texScale");
        cdFaces = field(CubeDefinition.class, "visibleFaces");
        gx = field(CubeDeformation.class, "growX");
        gy = field(CubeDeformation.class, "growY");
        gz = field(CubeDeformation.class, "growZ");
        mpChildren = field(ModelPart.class, "children");
        List<ModelLayerLocation> keys = new ArrayList<>(roots.keySet());
        keys.sort(Comparator.comparing(ModelLayerLocation::toString));
        out.append("{\"format\":1,\"layers\":[");
        for (int i = 0; i < keys.size(); i++) {
            ModelLayerLocation k = keys.get(i);
            LayerDefinition ld = roots.get(k);
            MeshDefinition mesh = (MeshDefinition) fMesh.get(ld);
            Object mat = fMat.get(ld);
            if (i > 0) out.append(',');
            out.append("{\"name\":\"").append(k).append("\",\"texture\":[").append(fTw.getInt(mat)).append(',')
                    .append(fTh.getInt(mat)).append("],\"root\":");
            part("root", mesh.getRoot(), ld.bakeRoot());
            out.append('}');
        }
        out.append("]}\n");
        Files.writeString(Path.of(args[0]), out, StandardCharsets.UTF_8);
        System.err.println(keys.size() + " entity model layers → " + args[0]);
        if (args.length > 1) animations(Path.of(args[1]));
        System.exit(0);
    }

    // Keyframe animations: static animation-definition fields of the classes in the definitions
    // package, found in the jar. Stable name "<class>.<field>" in lowercase, the class prefix
    // removed from the field (the camel's walk field → camel.walk). Vectors as stored: offsets
    // in pixels (y already inverted), in radians, scale − 1.
    static void animations(Path dst) throws Exception {
        String pkg = "net/minecraft/client/animation/definitions/";
        Path jar = Path.of(net.minecraft.client.animation.AnimationDefinition.class.getProtectionDomain()
                .getCodeSource().getLocation().toURI());
        List<String> classes = new ArrayList<>();
        try (java.util.zip.ZipFile z = new java.util.zip.ZipFile(jar.toFile())) {
            for (var en = z.entries(); en.hasMoreElements();) {
                String n = en.nextElement().getName();
                if (n.startsWith(pkg) && n.endsWith(".class") && n.indexOf('/', pkg.length()) < 0 && !n.contains("$"))
                    classes.add(n.substring(0, n.length() - 6).replace('/', '.'));
            }
        }
        Object lin = net.minecraft.client.animation.AnimationChannel.Interpolations.LINEAR;
        Object cat = net.minecraft.client.animation.AnimationChannel.Interpolations.CATMULLROM;
        List<Object> targets = List.of(net.minecraft.client.animation.AnimationChannel.Targets.POSITION,
                net.minecraft.client.animation.AnimationChannel.Targets.ROTATION,
                net.minecraft.client.animation.AnimationChannel.Targets.SCALE);
        TreeMap<String, net.minecraft.client.animation.AnimationDefinition> defs = new TreeMap<>();
        for (String cn : classes) {
            Class<?> c = Class.forName(cn);
            String prefix = c.getSimpleName().replaceAll("Animation$", "").replaceAll("([a-z0-9])([A-Z])", "$1_$2")
                    .toLowerCase(Locale.ROOT);
            for (Field f : c.getDeclaredFields()) {
                if (!java.lang.reflect.Modifier.isStatic(f.getModifiers())
                        || f.getType() != net.minecraft.client.animation.AnimationDefinition.class) continue;
                f.setAccessible(true);
                String n = f.getName().toLowerCase(Locale.ROOT);
                if (n.startsWith(prefix + "_")) n = n.substring(prefix.length() + 1);
                if (defs.put(prefix + "." + n, (net.minecraft.client.animation.AnimationDefinition) f.get(null)) != null)
                    throw new IllegalStateException("duplicate animation name: " + prefix + "." + n);
            }
        }
        StringBuilder o = new StringBuilder("{\"format\":1,\"animations\":[");
        boolean firstDef = true;
        for (var d : defs.entrySet()) {
            var def = d.getValue();
            if (!firstDef) o.append(',');
            firstDef = false;
            o.append("{\"name\":\"").append(d.getKey()).append("\",\"length\":").append(num(def.lengthInSeconds()))
                    .append(",\"looping\":").append(def.looping()).append(",\"channels\":[");
            boolean firstCh = true;
            for (var bone : new TreeMap<>(def.boneAnimations()).entrySet()) {
                for (var ch : bone.getValue()) {
                    int t = targets.indexOf(ch.target());
                    if (t < 0) throw new IllegalStateException("unknown target in " + d.getKey());
                    if (!firstCh) o.append(',');
                    firstCh = false;
                    o.append("{\"part\":\"").append(bone.getKey()).append("\",\"target\":").append(t).append(",\"keyframes\":[");
                    var kfs = ch.keyframes();
                    for (int i = 0; i < kfs.length; i++) {
                        var k = kfs[i];
                        Object ip = k.interpolation();
                        if (ip != lin && ip != cat) throw new IllegalStateException("unknown interpolation in " + d.getKey());
                        if (i > 0) o.append(',');
                        o.append("{\"t\":").append(num(k.timestamp())).append(",\"catmull\":").append(ip == cat)
                                .append(",\"pre\":").append(vec(k.preTarget())).append(",\"post\":").append(vec(k.postTarget()))
                                .append('}');
                    }
                    o.append("]}");
                }
            }
            o.append("]}");
        }
        o.append("]}\n");
        Files.writeString(dst, o, StandardCharsets.UTF_8);
        System.err.println(defs.size() + " entity animations → " + dst);
    }

    static String vec(org.joml.Vector3fc v) {
        return "[" + num(v.x()) + "," + num(v.y()) + "," + num(v.z()) + "]";
    }

    // Part: pose (x, y, z, x/y/z rotations in radians, scales), cubes, children in the game's
    // drawing order (iteration order of the baked part).
    @SuppressWarnings("unchecked")
    static void part(String name, PartDefinition pd, ModelPart mp) throws Exception {
        PartPose p = (PartPose) fPose.get(pd);
        out.append("{\"name\":\"").append(name).append("\",\"pose\":[").append(num(p.x())).append(',').append(num(p.y()))
                .append(',').append(num(p.z())).append(',').append(num(p.xRot())).append(',').append(num(p.yRot()))
                .append(',').append(num(p.zRot())).append(',').append(num(p.xScale())).append(',')
                .append(num(p.yScale())).append(',').append(num(p.zScale())).append("],\"cubes\":[");
        List<CubeDefinition> cubes = (List<CubeDefinition>) fCubes.get(pd);
        for (int i = 0; i < cubes.size(); i++) {
            CubeDefinition c = cubes.get(i);
            UVPair t = (UVPair) cdTex.get(c), s = (UVPair) cdScale.get(c);
            org.joml.Vector3fc o = (org.joml.Vector3fc) cdOrigin.get(c), d = (org.joml.Vector3fc) cdDim.get(c);
            Object g = cdGrow.get(c);
            Set<Direction> faces = (Set<Direction>) cdFaces.get(c);
            int mask = 0;
            for (Direction dir : faces) mask |= 1 << dir.ordinal();
            if (i > 0) out.append(',');
            out.append("{\"uv\":[").append(num(t.u())).append(',').append(num(t.v())).append("],\"from\":[")
                    .append(num(o.x())).append(',').append(num(o.y())).append(',').append(num(o.z())).append("],\"size\":[")
                    .append(num(d.x())).append(',').append(num(d.y())).append(',').append(num(d.z())).append("],\"grow\":[")
                    .append(num(gx.getFloat(g))).append(',').append(num(gy.getFloat(g))).append(',')
                    .append(num(gz.getFloat(g))).append("],\"mirror\":").append(cdMirror.getBoolean(c))
                    .append(",\"tex_scale\":[").append(num(s.u())).append(',').append(num(s.v()))
                    .append("],\"faces\":").append(mask).append('}');
        }
        out.append("],\"children\":[");
        Map<String, ModelPart> baked = (Map<String, ModelPart>) mpChildren.get(mp);
        boolean first = true;
        for (Map.Entry<String, ModelPart> en : baked.entrySet()) {
            if (!first) out.append(',');
            first = false;
            part(en.getKey(), pd.getChild(en.getKey()), en.getValue());
        }
        out.append("]}");
    }
}
