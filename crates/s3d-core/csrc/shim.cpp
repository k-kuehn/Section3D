// Thin C ABI over Manifold for the Rust side (crates/s3d-core/src/manifold.rs).
// Every entry point catches exceptions and reports failure as a null handle.

#include <cstdint>
#include <cstring>
#include <vector>

#include "manifold/manifold.h"

using manifold::Box;
using manifold::Manifold;
using manifold::MeshGL;
using manifold::OpType;
using manifold::Polygons;
using manifold::SimplePolygon;
using manifold::mat3x4;
using manifold::vec2;
using manifold::vec3;

struct S3dMf {
  Manifold m;
};

struct S3dMesh {
  MeshGL gl;
};

static S3dMf* wrap(Manifold&& m) { return new S3dMf{std::move(m)}; }

#define GUARD(expr, fail) \
  try {                   \
    expr;                 \
  } catch (...) {         \
    return fail;          \
  }

extern "C" {

void s3d_mf_free(S3dMf* h) { delete h; }
void s3d_mesh_free(S3dMesh* h) { delete h; }

S3dMf* s3d_mf_from_mesh(const float* verts, uint32_t nverts, int merge,
                        const uint32_t* tris, uint32_t ntris) {
  GUARD(
      {
        MeshGL gl;
        gl.numProp = 3;
        gl.vertProperties.assign(verts, verts + size_t(nverts) * 3);
        gl.triVerts.assign(tris, tris + size_t(ntris) * 3);
        if (merge) gl.Merge();
        return wrap(Manifold(gl));
      },
      nullptr)
}

S3dMf* s3d_mf_clone(const S3dMf* h) { GUARD(return wrap(Manifold(h->m)), nullptr) }

int s3d_mf_status(const S3dMf* h) { return static_cast<int>(h->m.Status()); }
int s3d_mf_is_empty(const S3dMf* h) { return h->m.IsEmpty() ? 1 : 0; }
uint64_t s3d_mf_num_tri(const S3dMf* h) { return h->m.NumTri(); }
uint64_t s3d_mf_num_vert(const S3dMf* h) { return h->m.NumVert(); }
double s3d_mf_volume(const S3dMf* h) { return h->m.Volume(); }
double s3d_mf_surface_area(const S3dMf* h) { return h->m.SurfaceArea(); }
int s3d_mf_genus(const S3dMf* h) { return h->m.Genus(); }

void s3d_mf_bbox(const S3dMf* h, double* out6) {
  Box b = h->m.BoundingBox();
  out6[0] = b.min.x; out6[1] = b.min.y; out6[2] = b.min.z;
  out6[3] = b.max.x; out6[4] = b.max.y; out6[5] = b.max.z;
}

S3dMesh* s3d_mf_get_mesh(const S3dMf* h) {
  GUARD(return new S3dMesh{h->m.GetMeshGL()}, nullptr)
}
uint32_t s3d_mesh_num_vert(const S3dMesh* m) { return m->gl.NumVert(); }
uint32_t s3d_mesh_num_tri(const S3dMesh* m) { return m->gl.NumTri(); }
// Copies positions (xyz per vertex, dropping any extra properties) and indices.
void s3d_mesh_copy(const S3dMesh* m, float* verts, uint32_t* tris) {
  const auto& gl = m->gl;
  const size_t nv = gl.NumVert(), np = gl.numProp;
  if (np == 3) {
    std::memcpy(verts, gl.vertProperties.data(), nv * 3 * sizeof(float));
  } else {
    for (size_t i = 0; i < nv; ++i)
      for (size_t k = 0; k < 3; ++k) verts[i * 3 + k] = gl.vertProperties[i * np + k];
  }
  std::memcpy(tris, gl.triVerts.data(), gl.triVerts.size() * sizeof(uint32_t));
}

S3dMf* s3d_mf_boolean(const S3dMf* a, const S3dMf* b, int op) {
  GUARD(return wrap(a->m.Boolean(b->m, static_cast<OpType>(op))), nullptr)
}

S3dMf* s3d_mf_batch_boolean(const S3dMf* const* hs, uint32_t n, int op) {
  GUARD(
      {
        std::vector<Manifold> v;
        v.reserve(n);
        for (uint32_t i = 0; i < n; ++i) v.push_back(hs[i]->m);
        return wrap(Manifold::BatchBoolean(v, static_cast<OpType>(op)));
      },
      nullptr)
}

S3dMf* s3d_mf_compose(const S3dMf* const* hs, uint32_t n) {
  GUARD(
      {
        std::vector<Manifold> v;
        v.reserve(n);
        for (uint32_t i = 0; i < n; ++i) v.push_back(hs[i]->m);
        return wrap(Manifold::Compose(v));
      },
      nullptr)
}

// Positive side (normal direction) goes to *out_pos.
int s3d_mf_split_by_plane(const S3dMf* h, double nx, double ny, double nz,
                          double offset, S3dMf** out_pos, S3dMf** out_neg) {
  GUARD(
      {
        auto r = h->m.SplitByPlane(vec3(nx, ny, nz), offset);
        *out_pos = wrap(std::move(r.first));
        *out_neg = wrap(std::move(r.second));
        return 1;
      },
      0)
}

S3dMf* s3d_mf_trim_by_plane(const S3dMf* h, double nx, double ny, double nz,
                            double offset) {
  GUARD(return wrap(h->m.TrimByPlane(vec3(nx, ny, nz), offset)), nullptr)
}

// Returns the number of components; *out receives a new[]'d array of handles
// that must be released with s3d_mf_free_array (handles themselves are owned
// by the caller).
uint32_t s3d_mf_decompose(const S3dMf* h, S3dMf*** out) {
  GUARD(
      {
        auto parts = h->m.Decompose();
        auto arr = new S3dMf*[parts.size()];
        for (size_t i = 0; i < parts.size(); ++i) arr[i] = wrap(std::move(parts[i]));
        *out = arr;
        return static_cast<uint32_t>(parts.size());
      },
      0)
}
void s3d_mf_free_array(S3dMf** arr) { delete[] arr; }

// m is a column-major 3x4 affine matrix (4 columns of xyz).
S3dMf* s3d_mf_transform(const S3dMf* h, const double* m) {
  GUARD(
      {
        mat3x4 t;
        for (int c = 0; c < 4; ++c) t[c] = vec3(m[c * 3], m[c * 3 + 1], m[c * 3 + 2]);
        return wrap(h->m.Transform(t));
      },
      nullptr)
}

S3dMf* s3d_mf_cube(double x, double y, double z, int center) {
  GUARD(return wrap(Manifold::Cube(vec3(x, y, z), center != 0)), nullptr)
}

S3dMf* s3d_mf_cylinder(double height, double r_low, double r_high, int segments,
                       int center) {
  GUARD(return wrap(Manifold::Cylinder(height, r_low, r_high, segments, center != 0)),
        nullptr)
}

S3dMf* s3d_mf_sphere(double r, int segments) {
  GUARD(return wrap(Manifold::Sphere(r, segments)), nullptr)
}

// xy: packed points of all contours; sizes: point count per contour.
S3dMf* s3d_mf_extrude(const double* xy, const uint32_t* sizes, uint32_t ncontours,
                      double height, double scale_top) {
  GUARD(
      {
        Polygons polys;
        size_t k = 0;
        for (uint32_t c = 0; c < ncontours; ++c) {
          SimplePolygon p;
          p.reserve(sizes[c]);
          for (uint32_t i = 0; i < sizes[c]; ++i, ++k) p.push_back(vec2(xy[k * 2], xy[k * 2 + 1]));
          polys.push_back(std::move(p));
        }
        return wrap(Manifold::Extrude(polys, height, 0, 0.0, vec2(scale_top, scale_top)));
      },
      nullptr)
}

S3dMf* s3d_mf_hull(const S3dMf* h) { GUARD(return wrap(h->m.Hull()), nullptr) }

S3dMf* s3d_mf_hull_points(const double* pts, uint32_t n) {
  GUARD(
      {
        std::vector<vec3> v(n);
        for (uint32_t i = 0; i < n; ++i) v[i] = vec3(pts[i * 3], pts[i * 3 + 1], pts[i * 3 + 2]);
        return wrap(Manifold::Hull(v));
      },
      nullptr)
}

S3dMf* s3d_mf_simplify(const S3dMf* h, double tolerance) {
  GUARD(return wrap(h->m.Simplify(tolerance)), nullptr)
}

typedef double (*S3dSdf)(double x, double y, double z, void* ctx);

// Surface where sdf == level; sdf is positive inside. Called from one thread
// only (MANIFOLD_PAR is off and canParallel is false).
S3dMf* s3d_mf_level_set(S3dSdf sdf, void* ctx, const double* bounds6,
                        double edge_length, double level) {
  try {
    Box b(vec3(bounds6[0], bounds6[1], bounds6[2]),
          vec3(bounds6[3], bounds6[4], bounds6[5]));
    auto f = [sdf, ctx](vec3 p) { return sdf(p.x, p.y, p.z, ctx); };
    return wrap(Manifold::LevelSet(f, b, edge_length, level, -1, false));
  } catch (...) {
    return nullptr;
  }
}

}  // extern "C"
