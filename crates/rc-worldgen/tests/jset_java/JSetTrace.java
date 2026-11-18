// Reference trace of the iteration order of a Java hash set (JDK only, no code
// of the game). Key: position whose hash and equality are those of the game's positions, subclass
// of a comparable class (like them: the tree of a bucket cannot use the comparison).
// Operation sequences drawn from a linear congruential generator shared with the Rust test `jset_java`.
//
//   java JSetTrace.java <output> <number of sequences> [sequence to detail]
//
// Output: one line "index fingerprint" per sequence (FNV-1a 64 of the results).
import java.io.PrintWriter;
import java.util.HashSet;
import java.util.Iterator;

public class JSetTrace {
    static class Base implements Comparable<Base> {
        final int x, y, z;

        Base(int x, int y, int z) {
            this.x = x;
            this.y = y;
            this.z = z;
        }

        @Override
        public boolean equals(Object o) {
            return o instanceof Base b && b.x == x && b.y == y && b.z == z;
        }

        @Override
        public int hashCode() {
            return (y + z * 31) * 31 + x;
        }

        @Override
        public int compareTo(Base o) {
            if (y != o.y) return y - o.y;
            return z != o.z ? z - o.z : x - o.x;
        }
    }

    static final class Pos extends Base {
        Pos(int x, int y, int z) {
            super(x, y, z);
        }
    }

    static long s;

    static int next() {
        s = s * 6364136223846793005L + 1442695040888963407L;
        return (int) (s >>> 33);
    }

    static int bounded(int n) {
        return next() % n;
    }

    static long h;

    static void mix(int v) {
        for (int i = 0; i < 4; i++) {
            h ^= (v >>> (8 * i)) & 0xff;
            h *= 0x100000001b3L;
        }
    }

    public static void main(String[] args) throws Exception {
        int count = Integer.parseInt(args[1]);
        int detail = args.length > 2 ? Integer.parseInt(args[2]) : -1;
        try (PrintWriter out = new PrintWriter(args[0], "UTF-8")) {
            for (int k = 0; k < count; k++) {
                s = k * 0x9E3779B97F4A7C15L + 12345L;
                h = 0xcbf29ce484222325L;
                int ox = bounded(2000000) - 1000000, oy = bounded(384) - 64, oz = bounded(2000000) - 1000000;
                int sx = 4 + bounded(21), sy = 4 + bounded(27), sz = 4 + bounded(21);
                int shape = bounded(3);
                if (shape == 1) sy = 1 + bounded(3);
                int ops = 200 + bounded(8000);
                int pop = new int[] {10, 35, 50}[bounded(3)];
                HashSet<Pos> set = new HashSet<>();
                for (int o = 0; o <= ops; o++) {
                    int r = o == ops ? 0 : bounded(100);
                    if (r < 2) {
                        mix(4);
                        mix(set.size());
                        for (Pos p : set) {
                            mix(p.x);
                            mix(p.y);
                            mix(p.z);
                        }
                    } else if (r < 2 + pop) {
                        Iterator<Pos> it = set.iterator();
                        if (it.hasNext()) {
                            Pos p = it.next();
                            it.remove();
                            mix(2);
                            mix(p.x);
                            mix(p.y);
                            mix(p.z);
                            if (k == detail) System.out.println("pop " + p.x + " " + p.y + " " + p.z);
                        } else {
                            mix(3);
                        }
                    } else if (r == 99 && bounded(20) == 0) {
                        set = new HashSet<>();
                        mix(5);
                    } else {
                        Pos p;
                        if (shape == 2) {
                            int d = bounded(24);
                            p = new Pos(ox + bounded(3) + d, oy + bounded(3) + d, oz + bounded(3) + 2 * d);
                        } else {
                            p = new Pos(ox + bounded(sx), oy + bounded(sy), oz + bounded(sz));
                        }
                        boolean added = set.add(p);
                        mix(1);
                        mix(added ? 1 : 0);
                        if (k == detail) System.out.println("add " + p.x + " " + p.y + " " + p.z + " " + added);
                    }
                }
                out.println(k + " " + Long.toUnsignedString(h, 16));
            }
        }
    }
}
