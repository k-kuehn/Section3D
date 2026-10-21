// Section3D viewport shaders.

struct Globals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    // xyz = normal, w = offset; section is on when params.y > 0.5
    clip: vec4<f32>,
    // x = sin(overhang limit), y = section on, z = viewport width px, w = viewport height px
    params: vec4<f32>,
    cap_color: vec4<f32>,
    support_color: vec4<f32>,
    // Lines are pulled towards the eye by x + y * distance (mm), so lines on
    // a surface win without showing through thin walls.
    line_bias: vec4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    // World (assembled) -> print frame, with the bed at z = 0.
    print: mat4x4<f32>,
    color: vec4<f32>,
    highlight: vec4<f32>,
    // x: bit0 support overlay, bit1 selected, bit2 ghost, bit3 hovered; y: first plane; z: plane count
    flags: vec4<u32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<storage, read> objects: array<Object>;
@group(0) @binding(2) var<storage, read> planes: array<vec4<f32>>;

struct MeshOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) local: vec3<f32>,
    @location(2) @interpolate(flat) obj: u32,
};

@vertex
fn vs_mesh(@location(0) p: vec3<f32>, @builtin(instance_index) obj: u32) -> MeshOut {
    let o = objects[obj];
    let w = o.model * vec4<f32>(p, 1.0);
    var out: MeshOut;
    out.pos = g.view_proj * w;
    out.world = w.xyz;
    out.local = p;
    out.obj = obj;
    return out;
}

fn to_gamma(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn shade(base: vec3<f32>, n: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let v = normalize(g.eye.xyz - world);
    let key = normalize(v + vec3<f32>(0.25, 0.15, 0.6));
    let fill = normalize(vec3<f32>(-0.5, -0.3, 0.4));
    let diff = max(dot(n, key), 0.0) * 0.75 + max(dot(n, fill), 0.0) * 0.18;
    let hemi = mix(0.22, 0.34, n.z * 0.5 + 0.5);
    let h = normalize(key + v);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * 0.12;
    return base * (hemi + diff) + vec3<f32>(spec);
}

@fragment
fn fs_mesh(in: MeshOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let o = objects[in.obj];
    if (g.params.y > 0.5 && dot(in.world, g.clip.xyz) > g.clip.w) {
        discard;
    }
    // Faceted normal from screen-space derivatives (faces the camera).
    var n = normalize(cross(dpdy(in.world), dpdx(in.world)));
    if (!front) {
        if (g.params.y > 0.5) {
            // Interior seen through the section plane: solid cap colour.
            let cn = -g.clip.xyz;
            let lit = 0.55 + 0.45 * abs(dot(normalize(g.eye.xyz - in.world), cn));
            return vec4<f32>(to_gamma(to_linear(g.cap_color.rgb) * lit), 1.0);
        }
    }
    let outward = select(-n, n, front);
    var base = to_linear(o.color.rgb);

    // Cut faces of the selected piece.
    if (o.highlight.a > 0.0) {
        let tol = 0.05 + length(in.local) * 2e-6;
        for (var i = 0u; i < o.flags.z; i = i + 1u) {
            let pl = planes[o.flags.y + i];
            if (abs(dot(pl.xyz, in.local) - pl.w) < tol && abs(dot(normalize(outward), pl.xyz)) > 0.995) {
                base = to_linear(o.highlight.rgb);
            }
        }
    }

    // Support overlay in print orientation.
    if ((o.flags.x & 1u) != 0u) {
        let np = normalize((o.print * vec4<f32>(outward, 0.0)).xyz);
        let pp = (o.print * vec4<f32>(in.local, 1.0)).xyz;
        if (np.z < -g.params.x && pp.z > 0.3) {
            base = mix(base, to_linear(g.support_color.rgb), 0.85);
        }
    }

    if ((o.flags.x & 2u) != 0u) {
        // Selected pieces take on the highlight colour a little.
        base = mix(base, to_linear(o.highlight.rgb), 0.22);
    }
    var c = shade(base, n, in.world);
    if ((o.flags.x & 2u) != 0u) {
        // Selected: lift and add a rim.
        let rim = pow(1.0 - abs(dot(n, normalize(g.eye.xyz - in.world))), 3.0);
        c = c * 1.12 + to_linear(o.highlight.rgb) * rim * 0.9;
    } else if ((o.flags.x & 8u) != 0u) {
        c = c * 1.15;
    }
    var a = o.color.a;
    if ((o.flags.x & 4u) != 0u) {
        a = 0.18;
    }
    return vec4<f32>(to_gamma(c), a);
}

// ---------------------------------------------------------------------------
// Thick lines: one instance per segment, expanded in screen space.

struct LineOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec3<f32>,
    @location(1) b: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) width: f32,
) -> LineOut {
    let da = length(g.eye.xyz - a);
    let db = length(g.eye.xyz - b);
    let a2 = a + (g.eye.xyz - a) / max(da, 1e-3) * (g.line_bias.x + g.line_bias.y * da);
    let b2 = b + (g.eye.xyz - b) / max(db, 1e-3) * (g.line_bias.x + g.line_bias.y * db);
    var ca = g.view_proj * vec4<f32>(a2, 1.0);
    var cb = g.view_proj * vec4<f32>(b2, 1.0);
    // Keep both ends in front of the near plane.
    let eps = 1e-4;
    if (ca.w < eps) { ca = mix(ca, cb, (eps - ca.w) / (cb.w - ca.w)); }
    if (cb.w < eps) { cb = mix(cb, ca, (eps - cb.w) / (ca.w - cb.w)); }
    let vp = g.params.zw;
    let sa = ca.xy / ca.w * vp * 0.5;
    let sb = cb.xy / cb.w * vp * 0.5;
    var dir = sb - sa;
    if (length(dir) < 1e-5) { dir = vec2<f32>(1.0, 0.0); }
    let nrm = normalize(vec2<f32>(-dir.y, dir.x)) * width * 0.5;
    let corners = array<vec2<f32>, 6>(vec2(0.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(0.0, -1.0), vec2(1.0, 1.0), vec2(0.0, 1.0));
    let c = corners[vi];
    let base = select(ca, cb, c.x > 0.5);
    let off = nrm * c.y / (vp * 0.5) * base.w;
    var out: LineOut;
    out.pos = vec4<f32>(base.xy + off, base.z, base.w);
    out.color = color;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return in.color;
}

// ---------------------------------------------------------------------------
// Translucent coloured triangles (cut planes, bed).

struct TriOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_tri(@location(0) p: vec3<f32>, @location(1) color: vec4<f32>) -> TriOut {
    var out: TriOut;
    out.pos = g.view_proj * vec4<f32>(p, 1.0);
    out.color = color;
    return out;
}

@fragment
fn fs_tri(in: TriOut) -> @location(0) vec4<f32> {
    return in.color;
}
