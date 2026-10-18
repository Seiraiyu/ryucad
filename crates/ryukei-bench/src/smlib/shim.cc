#include "ryukei-bench/src/smlib/shim.h"
#include "ryukei-bench/src/smlib.rs.h"

#include <SmAxis2Placement.h>
#include <SmBSplineCurve.h>
#include <SmBSplineSurface.h>
#include <SmBrep.h>
#include <SmEdge.h>
#include <SmFace.h>
#include <SmMerge.h>
#include <SmPoly.h>
#include <SmPrimitiveCreation.h>
#include <SmVertex.h>

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

// The breps before the context their objects live in.
Body::~Body() {
  delete tools;
  delete brep;
}

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

// Adds the primitive to `brep`'s infinite region.
static void create(SmBrep *brep, uint32_t kind, double a, double b, double c,
                   rust::Slice<const double> at) {
  SmPrimitiveCreation pc(brep->GetInfiniteRegion());
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
}

std::unique_ptr<Body> make(uint32_t kind, double a, double b, double c,
                           rust::Slice<const double> at, bool nurbs) {
  auto body = std::make_unique<Body>();
  body->brep = new (*body->context) SmBrep();
  create(body->brep, kind, a, b, c, at);
  if (nurbs) {
    to_bsplines(*body);
  }
  return body;
}

// The tools go into one brep of their own, in the body's context (SmMerge
// wants both breps in one), as separate lumps: they must not touch.
void add_tool(Body &body, uint32_t kind, double a, double b, double c,
              rust::Slice<const double> at) {
  auto *tool = new (*body.context) SmBrep();
  create(tool, kind, a, b, c, at);
  if (body.tools == nullptr) {
    body.tools = tool;
    return;
  }
  SmObjDelete cleanup(tool);
  check(body.tools->MergeBrep(*tool), "MergeBrep");
}

// SMLib's booleans and mesher run on TBB; cap it at one thread, for as
// long as `control` lives, unless parallel.
static void limit(std::optional<tbb::global_control> &control,
                  bool parallel) {
  if (!parallel) {
    control.emplace(tbb::global_control::max_allowed_parallelism, 1);
  }
}

// One difference with every tool at once, as with OCCT. SmMerge changes
// the body's brep in place and deletes the tools.
void subtract_tools(Body &body, bool parallel) {
  if (body.tools == nullptr) {
    throw std::runtime_error("no tools to subtract");
  }
  std::optional<tbb::global_control> one_thread;
  limit(one_thread, parallel);
  SmBrep *result = nullptr;
  SmMerge merge(*body.context, body.brep, body.tools);
  body.tools = nullptr;
  check(merge.ManifoldBoolean(SM_BO_DIFFERENCE, result), "ManifoldBoolean");
  if (result != body.brep) {
    throw std::runtime_error("ManifoldBoolean returned another brep");
  }
}

rust::Vec<double> vertices(const Body &body) {
  SmTArray<SmVertex *> vertices;
  body.brep->GetVertices(vertices);
  rust::Vec<double> out;
  for (ULONG i = 0; i < vertices.GetSize(); i++) {
    SmPoint3d p = vertices[i]->GetPoint();
    out.push_back(p.x);
    out.push_back(p.y);
    out.push_back(p.z);
  }
  return out;
}

uint32_t faces(const Body &body) {
  SmTArray<SmFace *> faces;
  body.brep->GetFaces(faces);
  return static_cast<uint32_t>(faces.GetSize());
}

SmlibMesh mesh(const Body &body, double chord, double angle,
               bool parallel) {
  std::optional<tbb::global_control> one_thread;
  limit(one_thread, parallel);
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
