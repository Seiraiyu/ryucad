#include "ryukei-bench/src/smlib/shim.h"
#include "ryukei-bench/src/smlib.rs.h"

#include <SmAxis2Placement.h>
#include <SmBSplineCurve.h>
#include <SmBSplineSurface.h>
#include <SmBrep.h>
#include <SmCurve.h>
#include <SmEdge.h>
#include <SmFace.h>
#include <SmFilletExecutive.h>
#include <SmFilletSolver.h>
#include <SmMerge.h>
#include <SmPoly.h>
#include <SmPrimitiveCreation.h>
#include <SmVertex.h>

#include <cmath>
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

// A profile's curves, as for the OCCT shim's `profile_face`, made the way
// SmPrimitiveCreation makes its own sweeps' (CreateLineSegment and
// CreateCircleSegment): SMLib finds the loops and their nesting itself.
static void profile_curves(const SmContext &context,
                           rust::Slice<const double> frame,
                           rust::Slice<const uint32_t> sizes,
                           rust::Slice<const double> segments,
                           SmTArray<SmCurve *> &out) {
  SmPoint3d o(frame[0], frame[1], frame[2]);
  SmVector3d x(frame[3], frame[4], frame[5]);
  SmVector3d y(frame[6], frame[7], frame[8]);
  auto at = [&](double u, double v) { return o + u * x + v * y; };
  size_t total = 0;
  for (uint32_t size : sizes) {
    total += size;
  }
  for (size_t k = 0; k < 6 * total; k += 6) {
    const double *s = &segments[k];
    SmBSplineCurve *curve = nullptr;
    if (s[0] == 0.0) {
      check(SmBSplineCurve::CreateLineSegment(context, 3, at(s[1], s[2]),
                                              at(s[3], s[4]), curve),
            "CreateLineSegment");
    } else {
      // A clockwise arc is a counterclockwise one about -y, angles negated.
      bool ccw = s[5] > s[4];
      SmAxis2Placement place(at(s[1], s[2]), x, ccw ? y : -y);
      double a0 = (ccw ? s[4] : -s[4]) * DEGREES;
      double a1 = (ccw ? s[5] : -s[5]) * DEGREES;
      check(SmBSplineCurve::CreateCircleSegment(context, 3, place, s[3], a0,
                                                a1, SM_CO_QUADRATIC, curve),
            "CreateCircleSegment");
    }
    out.Add(curve);
  }
}

std::unique_ptr<Body> extrude(rust::Slice<const double> frame,
                              rust::Slice<const uint32_t> sizes,
                              rust::Slice<const double> segments,
                              rust::Slice<const double> vector) {
  auto body = std::make_unique<Body>();
  body->brep = new (*body->context) SmBrep();
  SmTArray<SmCurve *> curves;
  profile_curves(*body->context, frame, sizes, segments, curves);
  SmVector3d v(vector[0], vector[1], vector[2]);
  double length = v.Length();
  SmPrimitiveCreation pc(body->brep->GetInfiniteRegion());
  check(pc.CreateLinearSweep(curves, v / length, length, 1, TRUE),
        "CreateLinearSweep");
  return body;
}

// Caps only for a partial revolve, as SmPrimitiveCreation's own cones and
// spheres do.
std::unique_ptr<Body> revolve(rust::Slice<const double> frame,
                              rust::Slice<const uint32_t> sizes,
                              rust::Slice<const double> segments,
                              rust::Slice<const double> axis, double angle) {
  auto body = std::make_unique<Body>();
  body->brep = new (*body->context) SmBrep();
  SmTArray<SmCurve *> curves;
  profile_curves(*body->context, frame, sizes, segments, curves);
  double degrees = angle * DEGREES;
  bool full = std::abs(degrees - 360.0) < 1e-9;
  SmPrimitiveCreation pc(body->brep->GetInfiniteRegion());
  check(pc.CreateRotationalSweep(curves, SmPoint3d(axis[0], axis[1], axis[2]),
                                 SmVector3d(axis[3], axis[4], axis[5]),
                                 degrees, 1, full ? FALSE : TRUE),
        "CreateRotationalSweep");
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

void subtract_tools(Body &body, bool parallel) {
  combine_tools(body, 2, parallel);
}

// One union (op 0), intersection (1) or difference (2) with every tool at
// once, as with OCCT. SmMerge changes the body's brep in place and
// deletes the tools.
void combine_tools(Body &body, uint32_t op, bool parallel) {
  if (body.tools == nullptr) {
    throw std::runtime_error("no tools to combine");
  }
  SmBooleanOperationType operation;
  switch (op) {
  case 0:
    operation = SM_BO_UNION;
    break;
  case 1:
    operation = SM_BO_INTERSECTION;
    break;
  case 2:
    operation = SM_BO_DIFFERENCE;
    break;
  default:
    throw std::runtime_error("no such boolean");
  }
  std::optional<tbb::global_control> one_thread;
  limit(one_thread, parallel);
  SmBrep *result = nullptr;
  SmMerge merge(*body.context, body.brep, body.tools);
  body.tools = nullptr;
  check(merge.ManifoldBoolean(operation, result), "ManifoldBoolean");
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

// Rounds the edges passing nearest each point of `edges` (x, y, z
// flattened, each edge sampled at 17 points), each by its radius, with one SmFilletExecutive: circular
// sections, constant radius, SMLib's default tolerances. The body's brep
// is filleted in place.
void fillet(Body &body, rust::Slice<const double> edges,
            rust::Slice<const double> radii, bool parallel) {
  SmTArray<SmEdge *> all;
  body.brep->GetEdges(all);
  SmTArray<SmEdge *> picked;
  SmTArray<SmFilletSurfaceGeneratorType> sections;
  SmTArray<SmFilletSolverType> solvers;
  SmTArray<double> sizes;
  for (size_t i = 0; i < radii.size(); i++) {
    SmPoint3d at(edges[3 * i], edges[3 * i + 1], edges[3 * i + 2]);
    SmEdge *best = nullptr;
    double least = 1e300;
    for (ULONG k = 0; k < all.GetSize(); k++) {
      SmCurve *c = all[k]->GetCurve();
      if (c == nullptr) {
        continue;
      }
      SmExtent1d ivl = all[k]->GetInterval();
      for (int j = 0; j <= 16; j++) {
        SmPoint3d m;
        double t = ivl.GetMin() + (ivl.GetMax() - ivl.GetMin()) * j / 16.0;
        check(c->EvaluatePoint(t, m), "EvaluatePoint");
        double d = (m - at).Length();
        if (d < least) {
          least = d;
          best = all[k];
        }
      }
    }
    if (best == nullptr) {
      throw std::runtime_error("no edge");
    }
    picked.Add(best);
    sections.Add(SM_FSG_CIRCULAR);
    solvers.Add(SM_FS_CONST_RADIUS);
    sizes.Add(radii[i]);
  }
  std::optional<tbb::global_control> one_thread;
  limit(one_thread, parallel);
  SmFilletExecutive ex(*body.context, body.brep);
  check(ex.SetFilletParameters(body.brep, picked, sections, solvers, sizes,
                               nullptr, 0.0, 0.0, 0.0, 0.0),
        "SetFilletParameters");
  check(ex.DoFillet(), "DoFillet");
}

} // namespace ryukei_smlib
