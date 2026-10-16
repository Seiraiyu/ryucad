#include "ryukei-bench/src/smlib/shim.h"
#include "ryukei-bench/src/smlib.rs.h"

#include <SmAxis2Placement.h>
#include <SmBSplineCurve.h>
#include <SmBSplineSurface.h>
#include <SmBrep.h>
#include <SmEdge.h>
#include <SmFace.h>
#include <SmPoly.h>
#include <SmPrimitiveCreation.h>

#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <tbb/global_control.h>
#include <unordered_map>

namespace ryukei_smlib {

static constexpr double DEGREES = 180.0 / 3.14159265358979323846;

static void check(SmStatus s, const char *what) {
  if (s != SM_SUCCESS) {
    throw std::runtime_error(std::string(what) + " failed with status " +
                             std::to_string(static_cast<long>(s)));
  }
}

Body::Body() : context(std::make_unique<SmContext>()) {}

// The brep before the context its objects live in.
Body::~Body() { delete brep; }

// Every surface and curve replaced by a plain B-spline copy, dropping
// the analytic subclass (SMLib's analytic types are B-splines already).
static void to_bsplines(Body &body) {
  SmTArray<SmFace *> faces;
  body.brep->GetFaces(faces);
  for (ULONG i = 0; i < faces.GetSize(); i++) {
    auto *s = new (*body.context) SmBSplineSurface(
        *static_cast<SmBSplineSurface *>(faces[i]->GetSurface()));
    check(body.brep->ReplaceSurface(faces[i], s, TRUE), "ReplaceSurface");
  }
  SmTArray<SmEdge *> edges;
  body.brep->GetEdges(edges);
  for (ULONG i = 0; i < edges.GetSize(); i++) {
    if (edges[i]->GetCurve() == nullptr) {
      continue;
    }
    auto *c = new (*body.context) SmBSplineCurve(
        *static_cast<SmBSplineCurve *>(edges[i]->GetCurve()));
    check(body.brep->ReplaceCurve(edges[i], c), "ReplaceCurve");
  }
}

std::unique_ptr<Body> make(uint32_t kind, double a, double b, double c,
                           rust::Slice<const double> at, bool nurbs) {
  auto body = std::make_unique<Body>();
  body->brep = new (*body->context) SmBrep();
  SmPrimitiveCreation pc(body->brep->GetInfiniteRegion());
  SmAxis2Placement frame(at[0], at[1], at[2], at[3], at[4], at[5], at[6],
                         at[7], at[8]);
  switch (kind) {
  case 0:
    check(pc.CreateBox(a, b, c, frame), "CreateBox");
    break;
  case 1:
    check(pc.CreateCone(c, a, b, 0.0, 360.0, frame), "CreateCone");
    break;
  case 2:
    check(pc.CreateSphere(a, 0.0, 360.0, frame), "CreateSphere");
    break;
  case 3:
    check(pc.CreateTorus(a, b, 0.0, 360.0, frame), "CreateTorus");
    break;
  default:
    throw std::runtime_error("unknown primitive kind");
  }
  if (nurbs) {
    to_bsplines(*body);
  }
  return body;
}

SmlibMesh mesh(const Body &body, double chord, double angle,
               bool parallel) {
  // SMLib meshes faces with TBB; cap it at one thread unless parallel.
  std::optional<tbb::global_control> one_thread;
  if (!parallel) {
    one_thread.emplace(tbb::global_control::max_allowed_parallelism, 1);
  }
  // ConvertToPolyBrep meshes a copy, so the body is not changed.
  SmPolyBrep *poly = nullptr;
  SmBoolean failed = FALSE;
  ULONG lamina = 0;
  check(body.brep->ConvertToPolyBrep(poly, failed, lamina, chord,
                                     angle * DEGREES, angle * DEGREES),
        "ConvertToPolyBrep");
  SmObjDelete cleanup(poly);

  SmlibMesh out;
  out.failed_faces = failed != FALSE;
  out.lamina_edges = static_cast<uint32_t>(lamina);
  SmTArray<SmPolyVertex *> vertices;
  poly->GetPolyVertices(vertices);
  std::unordered_map<const SmPolyVertex *, uint32_t> index;
  for (ULONG i = 0; i < vertices.GetSize(); i++) {
    index[vertices[i]] = static_cast<uint32_t>(i);
    SmPoint3d p = vertices[i]->GetPoint();
    out.positions.push_back(p.x);
    out.positions.push_back(p.y);
    out.positions.push_back(p.z);
  }
  SmTArray<SmPolyFace *> faces;
  poly->GetPolyFaces(faces);
  for (ULONG i = 0; i < faces.GetSize(); i++) {
    SmTArray<SmPolyLoop *> loops;
    faces[i]->GetPolyLoops(loops);
    if (loops.GetSize() != 1) {
      throw std::runtime_error("a polygon with holes");
    }
    SmTArray<SmPolyVertex *> ring;
    loops[0]->GetPolyVertices(ring);
    if (ring.GetSize() != 3) {
      out.polygons += 1;
    }
    for (ULONG k = 1; k + 1 < ring.GetSize(); k++) {
      for (const SmPolyVertex *v : {ring[0], ring[k], ring[k + 1]}) {
        out.triangles.push_back(index.at(v));
      }
    }
  }
  return out;
}

} // namespace ryukei_smlib
