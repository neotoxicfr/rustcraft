// Communication of the faces of a 16³ section (same rule as `visibility::section_vis`), one workgroup
// per section: propagation of the minimal label through the non-solid cells,
// then union of the section faces touched by each zone.

@group(0) @binding(0) var<storage, read> solid_in: array<u32>;   // 128 words per section
@group(0) @binding(1) var<storage, read_write> out: array<vec2<u32>>;

var<workgroup> sbits: array<u32, 128>;
var<workgroup> lab: array<atomic<u32>, 4096>;
var<workgroup> faces: array<atomic<u32>, 1024>;   // 8 bits per root cell
var<workgroup> count: atomic<u32>;
var<workgroup> changed: atomic<u32>;
var<workgroup> flag: u32;
var<workgroup> vis: array<atomic<u32>, 2>;

const NONE: u32 = 0xffffffffu;

fn is_solid(i: u32) -> bool {
    return ((sbits[i >> 5u] >> (i & 31u)) & 1u) == 1u;
}

fn face_bits(i: u32) -> u32 {
    let x = i & 15u;
    let y = i >> 8u;
    let z = (i >> 4u) & 15u;
    var f = 0u;
    if x == 0u { f |= 16u; } else if x == 15u { f |= 32u; }
    if y == 0u { f |= 1u; } else if y == 15u { f |= 2u; }
    if z == 0u { f |= 4u; } else if z == 15u { f |= 8u; }
    return f;
}

@compute @workgroup_size(256)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let section = wg.x;
    if t < 128u {
        sbits[t] = solid_in[section * 128u + t];
    }
    if t == 0u {
        atomicStore(&count, 0u);
        atomicStore(&vis[0], 0u);
        atomicStore(&vis[1], 0u);
    }
    workgroupBarrier();
    if t < 128u {
        atomicAdd(&count, countOneBits(sbits[t]));
    }
    for (var k = 0u; k < 4u; k++) {
        atomicStore(&faces[t * 4u + k], 0u);
    }
    for (var k = 0u; k < 16u; k++) {
        let i = t * 16u + k;
        atomicStore(&lab[i], select(i, NONE, is_solid(i)));
    }
    workgroupBarrier();
    if t == 0u {
        flag = atomicLoad(&count);
    }
    let opaque = workgroupUniformLoad(&flag);
    if opaque < 256u {
        if t == 0u { out[section] = vec2<u32>(0xffffffffu, 0xfu); }
        return;
    }
    if opaque == 4096u {
        if t == 0u { out[section] = vec2<u32>(0u, 0u); }
        return;
    }
    loop {
        if t == 0u { atomicStore(&changed, 0u); }
        workgroupBarrier();
        for (var k = 0u; k < 16u; k++) {
            let i = t * 16u + k;
            if is_solid(i) { continue; }
            let cur = atomicLoad(&lab[i]);
            var m = cur;
            let x = i & 15u;
            let y = i >> 8u;
            let z = (i >> 4u) & 15u;
            if x > 0u && !is_solid(i - 1u) { m = min(m, atomicLoad(&lab[i - 1u])); }
            if x < 15u && !is_solid(i + 1u) { m = min(m, atomicLoad(&lab[i + 1u])); }
            if z > 0u && !is_solid(i - 16u) { m = min(m, atomicLoad(&lab[i - 16u])); }
            if z < 15u && !is_solid(i + 16u) { m = min(m, atomicLoad(&lab[i + 16u])); }
            if y > 0u && !is_solid(i - 256u) { m = min(m, atomicLoad(&lab[i - 256u])); }
            if y < 15u && !is_solid(i + 256u) { m = min(m, atomicLoad(&lab[i + 256u])); }
            // Pointer jump: the label of the label (shortens the propagation).
            m = min(m, atomicLoad(&lab[m]));
            if m < cur {
                atomicMin(&lab[i], m);
                atomicStore(&changed, 1u);
            }
        }
        workgroupBarrier();
        if t == 0u { flag = atomicLoad(&changed); }
        if workgroupUniformLoad(&flag) == 0u { break; }
    }
    // Faces touched by each zone, stored at its root (smallest index of the zone).
    for (var k = 0u; k < 16u; k++) {
        let i = t * 16u + k;
        let f = face_bits(i);
        if f != 0u && !is_solid(i) {
            let r = atomicLoad(&lab[i]);
            atomicOr(&faces[r >> 2u], f << ((r & 3u) * 8u));
        }
    }
    workgroupBarrier();
    for (var k = 0u; k < 16u; k++) {
        let r = t * 16u + k;
        let f = (atomicLoad(&faces[r >> 2u]) >> ((r & 3u) * 8u)) & 63u;
        if f == 0u { continue; }
        var lo = 0u;
        var hi = 0u;
        for (var a = 0u; a < 6u; a++) {
            if ((f >> a) & 1u) == 1u {
                let s = a * 6u;
                lo |= f << s;
                if s + 6u > 32u { hi |= f >> (32u - s); }
            }
        }
        atomicOr(&vis[0], lo);
        atomicOr(&vis[1], hi);
    }
    workgroupBarrier();
    if t == 0u {
        out[section] = vec2<u32>(atomicLoad(&vis[0]), atomicLoad(&vis[1]));
    }
}
