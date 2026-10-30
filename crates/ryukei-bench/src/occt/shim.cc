#include "ryukei-bench/src/occt/shim.h"
#include "ryukei-bench/src/occt.rs.h"

#include <BRepAlgoAPI_BooleanOperation.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Section.hxx>
#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeVertex.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepBuilderAPI_NurbsConvert.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepAdaptor_Curve.hxx>
#include <BRepExtrema_DistShapeShape.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <BRepGProp.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepOffsetAPI_MakeOffsetShape.hxx>
#include <BRepOffsetAPI_MakePipeShell.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <BRepOffsetAPI_ThruSections.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <Interface_Static.hxx>
#include <STEPControl_Reader.hxx>
#include <STEPControl_Writer.hxx>
#include <RWStl.hxx>
#include <ShapeFix_Shape.hxx>
#include <Standard_Failure.hxx>
#include <GProp_GProps.hxx>
#include <Geom_BSplineCurve.hxx>
#include <TColStd_Array1OfInteger.hxx>
#include <TColStd_Array1OfReal.hxx>
#include <TColgp_Array1OfPnt.hxx>
#include <Poly_Triangulation.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Iterator.hxx>
#include <gp_Ax3.hxx>
#include <gp_Circ.hxx>
#include <gp_Pln.hxx>
#include <gp_Trsf.hxx>

#include <optional>
#include <vector>
#include <stdexcept>
#include <string>

namespace ryukei_occt {

static std::unique_ptr<Shape> wrap(const TopoDS_Shape &s) {
  return std::make_unique<Shape>(s);
}

std::unique_ptr<Shape> make_box(double dx, double dy, double dz) {
  return wrap(BRepPrimAPI_MakeBox(dx, dy, dz).Shape());
}

std::unique_ptr<Shape> make_cylinder(double r, double h) {
  return wrap(BRepPrimAPI_MakeCylinder(r, h).Shape());
}

std::unique_ptr<Shape> make_cone(double r0, double r1, double h) {
  return wrap(BRepPrimAPI_MakeCone(r0, r1, h).Shape());
}

std::unique_ptr<Shape> make_sphere(double r) {
  return wrap(BRepPrimAPI_MakeSphere(r).Shape());
}

std::unique_ptr<Shape> make_torus(double major, double minor) {
  return wrap(BRepPrimAPI_MakeTorus(major, minor).Shape());
}

// m: a row-major 3 x 3 rotation, then the translation.
std::unique_ptr<Shape> placed(const Shape &s, rust::Slice<const double> m) {
  gp_Trsf t;
  t.SetValues(m[0], m[1], m[2], m[9], m[3], m[4], m[5], m[10], m[6], m[7],
              m[8], m[11]);
  return wrap(BRepBuilderAPI_Transform(s.shape, t, true).Shape());
}

std::unique_ptr<Shape> nurbs(const Shape &s) {
  return wrap(BRepBuilderAPI_NurbsConvert(s.shape, true).Shape());
}

std::unique_ptr<Shape> make_compound() {
  TopoDS_Compound c;
  BRep_Builder().MakeCompound(c);
  return wrap(c);
}

void add_to_compound(Shape &compound, const Shape &s) {
  BRep_Builder().Add(compound.shape, s.shape);
}

// One cut with every member of the compound `tools` as a tool, which is
// how OCCT's Boolean Operations take many tools: cutting them one at a
// time would intersect the growing result again at every step.
std::unique_ptr<Shape> cut(const Shape &s, const Shape &tools, bool parallel) {
  return boolean(s, tools, 2, parallel);
}

// `s` united with (op 0), intersected with (1) or minus (2) every solid of
// the compound `tools`, in one boolean with the tools as its tool list.
std::unique_ptr<Shape> boolean(const Shape &s, const Shape &tools, uint32_t op,
                               bool parallel) {
  TopTools_ListOfShape arguments, list;
  arguments.Append(s.shape);
  for (TopoDS_Iterator it(tools.shape); it.More(); it.Next()) {
    list.Append(it.Value());
  }
  BRepAlgoAPI_BooleanOperation bop;
  switch (op) {
  case 0:
    bop.SetOperation(BOPAlgo_FUSE);
    break;
  case 1:
    bop.SetOperation(BOPAlgo_COMMON);
    break;
  case 2:
    bop.SetOperation(BOPAlgo_CUT);
    break;
  default:
    throw std::runtime_error("no such boolean");
  }
  bop.SetArguments(arguments);
  bop.SetTools(list);
  bop.SetRunParallel(parallel);
  bop.Build();
  if (!bop.IsDone() || bop.HasErrors()) {
    throw std::runtime_error("BRepAlgoAPI_BooleanOperation failed");
  }
  return wrap(bop.Shape());
}

// The B-rep's vertex positions (x, y, z flattened), in TopExp's order.
rust::Vec<double> vertices(const Shape &s) {
  TopTools_IndexedMapOfShape map;
  TopExp::MapShapes(s.shape, TopAbs_VERTEX, map);
  rust::Vec<double> out;
  for (int i = 1; i <= map.Extent(); ++i) {
    gp_Pnt p = BRep_Tool::Pnt(TopoDS::Vertex(map(i)));
    out.push_back(p.X());
    out.push_back(p.Y());
    out.push_back(p.Z());
  }
  return out;
}

uint32_t faces(const Shape &s) {
  TopTools_IndexedMapOfShape map;
  TopExp::MapShapes(s.shape, TopAbs_FACE, map);
  return static_cast<uint32_t>(map.Extent());
}

// The face bounded by a profile: `frame` the plane's origin and its x and
// y axes; `sizes` each loop's segment count, the outer loop first (and
// counterclockwise), then holes (clockwise); `segments` six numbers per
// segment: 0 and a line's ends, or 1 and an arc's centre, radius and
// angles (counterclockwise if the second is larger).
static TopoDS_Face profile_face(rust::Slice<const double> frame,
                                rust::Slice<const uint32_t> sizes,
                                rust::Slice<const double> segments) {
  gp_Pnt o(frame[0], frame[1], frame[2]);
  gp_Dir x(frame[3], frame[4], frame[5]);
  gp_Dir y(frame[6], frame[7], frame[8]);
  gp_Dir n = x.Crossed(y);
  auto at = [&](double u, double v) {
    return o.Translated(gp_Vec(x) * u + gp_Vec(y) * v);
  };
  std::optional<BRepBuilderAPI_MakeFace> face;
  size_t k = 0;
  for (uint32_t size : sizes) {
    BRepBuilderAPI_MakeWire wire;
    for (uint32_t i = 0; i < size; ++i, k += 6) {
      const double *s = &segments[k];
      TopoDS_Edge edge;
      if (s[0] == 0.0) {
        edge = BRepBuilderAPI_MakeEdge(at(s[1], s[2]), at(s[3], s[4])).Edge();
      } else {
        gp_Circ circle(gp_Ax2(at(s[1], s[2]), n, x), s[3]);
        if (s[5] > s[4]) {
          edge = BRepBuilderAPI_MakeEdge(circle, s[4], s[5]).Edge();
        } else {
          edge = TopoDS::Edge(
              BRepBuilderAPI_MakeEdge(circle, s[5], s[4]).Edge().Reversed());
        }
      }
      wire.Add(edge);
    }
    if (!wire.IsDone()) {
      throw std::runtime_error("BRepBuilderAPI_MakeWire failed");
    }
    if (!face) {
      face.emplace(gp_Pln(gp_Ax3(o, n, x)), wire.Wire(), true);
    } else {
      face->Add(wire.Wire());
    }
  }
  if (!face || !face->IsDone()) {
    throw std::runtime_error("BRepBuilderAPI_MakeFace failed");
  }
  return face->Face();
}

std::unique_ptr<Shape> extrude(rust::Slice<const double> frame,
                               rust::Slice<const uint32_t> sizes,
                               rust::Slice<const double> segments,
                               rust::Slice<const double> vector) {
  TopoDS_Face face = profile_face(frame, sizes, segments);
  BRepPrimAPI_MakePrism prism(face, gp_Vec(vector[0], vector[1], vector[2]));
  if (!prism.IsDone()) {
    throw std::runtime_error("BRepPrimAPI_MakePrism failed");
  }
  return wrap(prism.Shape());
}

std::unique_ptr<Shape> revolve(rust::Slice<const double> frame,
                               rust::Slice<const uint32_t> sizes,
                               rust::Slice<const double> segments,
                               rust::Slice<const double> axis, double angle) {
  TopoDS_Face face = profile_face(frame, sizes, segments);
  gp_Ax1 a(gp_Pnt(axis[0], axis[1], axis[2]), gp_Dir(axis[3], axis[4], axis[5]));
  BRepPrimAPI_MakeRevol revol(face, a, angle);
  if (!revol.IsDone()) {
    throw std::runtime_error("BRepPrimAPI_MakeRevol failed");
  }
  return wrap(revol.Shape());
}

OcctMesh mesh(const Shape &s, double chord, double angle, bool parallel) {
  // Each call meshes afresh.
  TopoDS_Shape copy = BRepBuilderAPI_Transform(s.shape, gp_Trsf(), true).Shape();
  BRepMesh_IncrementalMesh(copy, chord, false, angle, parallel);
  OcctMesh out;
  for (TopExp_Explorer ex(copy, TopAbs_FACE); ex.More(); ex.Next()) {
    const TopoDS_Face &face = TopoDS::Face(ex.Current());
    TopLoc_Location loc;
    Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(face, loc);
    if (tri.IsNull()) {
      out.faces_without_mesh += 1;
      continue;
    }
    const gp_Trsf &t = loc.Transformation();
    uint32_t base = static_cast<uint32_t>(out.positions.size() / 3);
    for (int i = 1; i <= tri->NbNodes(); ++i) {
      gp_Pnt p = tri->Node(i).Transformed(t);
      out.positions.push_back(p.X());
      out.positions.push_back(p.Y());
      out.positions.push_back(p.Z());
    }
    bool reversed = face.Orientation() == TopAbs_REVERSED;
    for (int i = 1; i <= tri->NbTriangles(); ++i) {
      int a, b, c;
      tri->Triangle(i).Get(a, b, c);
      if (reversed) {
        std::swap(b, c);
      }
      for (int v : {a, b, c}) {
        out.triangles.push_back(base + static_cast<uint32_t>(v - 1));
      }
    }
  }
  return out;
}

double volume(const Shape &s) {
  GProp_GProps props;
  BRepGProp::VolumeProperties(s.shape, props);
  return props.Mass();
}

double area(const Shape &s) {
  GProp_GProps props;
  BRepGProp::SurfaceProperties(s.shape, props);
  return props.Mass();
}

// To the shell, not the solid: a point inside a solid is at distance 0
// from it.
double distance(const Shape &s, double x, double y, double z) {
  TopoDS_Shape v = BRepBuilderAPI_MakeVertex(gp_Pnt(x, y, z)).Shape();
  TopExp_Explorer shell(s.shape, TopAbs_SHELL);
  BRepExtrema_DistShapeShape d(shell.More() ? shell.Current() : s.shape, v);
  return d.Value();
}

// The edge passing nearest (x, y, z), sampled at 17 points.
static TopoDS_Edge edge_at(const TopoDS_Shape &s, double x, double y,
                           double z) {
  TopoDS_Edge best;
  double least = 1e300;
  for (TopExp_Explorer ex(s, TopAbs_EDGE); ex.More(); ex.Next()) {
    const TopoDS_Edge &e = TopoDS::Edge(ex.Current());
    if (BRep_Tool::Degenerated(e)) {
      continue;
    }
    BRepAdaptor_Curve c(e);
    double t0 = c.FirstParameter(), t1 = c.LastParameter();
    for (int k = 0; k <= 16; k++) {
      double d = c.Value(t0 + (t1 - t0) * k / 16.0).Distance(gp_Pnt(x, y, z));
      if (d < least) {
        least = d;
        best = e;
      }
    }
  }
  if (best.IsNull()) {
    throw std::runtime_error("no edge");
  }
  return best;
}

// Of the faces either side of `e`, the one nearest (x, y, z).
static TopoDS_Face face_at(const TopoDS_Shape &s, const TopoDS_Edge &e,
                           double x, double y, double z) {
  TopTools_IndexedDataMapOfShapeListOfShape map;
  TopExp::MapShapesAndAncestors(s, TopAbs_EDGE, TopAbs_FACE, map);
  TopoDS_Face best;
  double least = 1e300;
  TopoDS_Vertex v = BRepBuilderAPI_MakeVertex(gp_Pnt(x, y, z));
  for (const TopoDS_Shape &f : map.FindFromKey(e)) {
    BRepExtrema_DistShapeShape d(v, f);
    if (d.IsDone() && d.Value() < least) {
      least = d.Value();
      best = TopoDS::Face(f);
    }
  }
  if (best.IsNull()) {
    throw std::runtime_error("no face");
  }
  return best;
}

// edges: a point on or near each edge (x, y, z flattened); radii: one
// each.
std::unique_ptr<Shape> fillet(const Shape &s, rust::Slice<const double> edges,
                              rust::Slice<const double> radii) {
  try {
    BRepFilletAPI_MakeFillet mk(s.shape);
    for (size_t i = 0; i < radii.size(); i++) {
      mk.Add(radii[i], edge_at(s.shape, edges[3 * i], edges[3 * i + 1],
                               edges[3 * i + 2]));
    }
    mk.Build();
    if (!mk.IsDone()) {
      throw std::runtime_error("BRepFilletAPI_MakeFillet failed");
    }
    return wrap(mk.Shape());
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("BRepFilletAPI_MakeFillet: ") +
                             e.GetMessageString());
  }
}

// modes: 0 equal (sizes[2i] each way), 1 two distances (sizes[2i] on the
// face nearest faces[3i..], sizes[2i + 1] on the other), 2 distance and
// angle in radians (from the face nearest faces[3i..]).
std::unique_ptr<Shape> chamfer(const Shape &s, rust::Slice<const double> edges,
                               rust::Slice<const uint32_t> modes,
                               rust::Slice<const double> sizes,
                               rust::Slice<const double> faces) {
  try {
    BRepFilletAPI_MakeChamfer mk(s.shape);
    for (size_t i = 0; i < modes.size(); i++) {
      TopoDS_Edge e =
          edge_at(s.shape, edges[3 * i], edges[3 * i + 1], edges[3 * i + 2]);
      double a = sizes[2 * i], b = sizes[2 * i + 1];
      if (modes[i] == 0) {
        mk.Add(a, e);
        continue;
      }
      TopoDS_Face f =
          face_at(s.shape, e, faces[3 * i], faces[3 * i + 1], faces[3 * i + 2]);
      if (modes[i] == 1) {
        mk.Add(a, b, e, f);
      } else {
        mk.AddDA(a, b, e, f);
      }
    }
    mk.Build();
    if (!mk.IsDone()) {
      throw std::runtime_error("BRepFilletAPI_MakeChamfer failed");
    }
    return wrap(mk.Shape());
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("BRepFilletAPI_MakeChamfer: ") +
                             e.GetMessageString());
  }
}

// Every root of the STEP file at `path`, as one shape, through
// STEPControl_Reader, then ShapeFix_Shape if `fix`.
std::unique_ptr<Shape> step_read(rust::Str path, bool fix) {
  try {
    STEPControl_Reader reader;
    std::string p(path);
    if (reader.ReadFile(p.c_str()) != IFSelect_RetDone) {
      throw std::runtime_error("STEPControl_Reader could not read " + p);
    }
    reader.TransferRoots();
    TopoDS_Shape shape = reader.OneShape();
    if (fix) {
      ShapeFix_Shape fixer(shape);
      fixer.Perform();
      shape = fixer.Shape();
    }
    return wrap(shape);
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("STEPControl_Reader: ") +
                             e.GetMessageString());
  }
}

// The shape written to `path` as AP242 (or AP214) through
// STEPControl_Writer.
void step_write(const Shape &s, rust::Str path, bool ap242) {
  try {
    Interface_Static::SetCVal("write.step.schema", ap242 ? "AP242DIS" : "AP214IS");
    STEPControl_Writer writer;
    if (writer.Transfer(s.shape, STEPControl_AsIs) != IFSelect_RetDone) {
      throw std::runtime_error("STEPControl_Writer could not transfer");
    }
    std::string p(path);
    if (writer.Write(p.c_str()) != IFSelect_RetDone) {
      throw std::runtime_error("STEPControl_Writer could not write " + p);
    }
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("STEPControl_Writer: ") +
                             e.GetMessageString());
  }
}

// The STL file at `path` through RWStl::ReadFile, coincident vertices
// welded: positions and triangles as position indices.
OcctMesh stl_read(rust::Str path) {
  try {
    std::string p(path);
    Handle(Poly_Triangulation) t = RWStl::ReadFile(p.c_str());
    if (t.IsNull()) {
      throw std::runtime_error("RWStl could not read " + p);
    }
    OcctMesh out;
    for (Standard_Integer i = 1; i <= t->NbNodes(); ++i) {
      gp_Pnt q = t->Node(i);
      out.positions.push_back(q.X());
      out.positions.push_back(q.Y());
      out.positions.push_back(q.Z());
    }
    for (Standard_Integer i = 1; i <= t->NbTriangles(); ++i) {
      Standard_Integer a, b, c;
      t->Triangle(i).Get(a, b, c);
      out.triangles.push_back(uint32_t(a - 1));
      out.triangles.push_back(uint32_t(b - 1));
      out.triangles.push_back(uint32_t(c - 1));
    }
    out.faces_without_mesh = 0;
    return out;
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("RWStl: ") + e.GetMessageString());
  }
}

// Each solid of the shape, in OCCT's order: volume, area, face count.
rust::Vec<double> solid_props(const Shape &s) {
  rust::Vec<double> out;
  for (TopExp_Explorer x(s.shape, TopAbs_SOLID); x.More(); x.Next()) {
    GProp_GProps v, a;
    BRepGProp::VolumeProperties(x.Current(), v);
    BRepGProp::SurfaceProperties(x.Current(), a);
    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(x.Current(), TopAbs_FACE, faces);
    out.push_back(v.Mass());
    out.push_back(a.Mass());
    out.push_back(double(faces.Extent()));
  }
  return out;
}

// A clamped B-spline curve from its encoding: degree, distinct knots,
// their multiplicities, poles (x, y, z flattened) and weights (empty for
// a polynomial curve). `at` walks `data`; `spec` gives the counts.
static Handle(Geom_BSplineCurve) bspline(rust::Slice<const uint32_t> spec,
                                         size_t &s, rust::Slice<const double> data,
                                         size_t &at) {
  const int degree = int(spec[s]), nk = int(spec[s + 1]), np = int(spec[s + 2]);
  const bool rational = spec[s + 3] != 0;
  s += 4;
  TColStd_Array1OfReal knots(1, nk);
  TColStd_Array1OfInteger mults(1, nk);
  for (int i = 1; i <= nk; ++i) {
    knots(i) = data[at++];
  }
  for (int i = 1; i <= nk; ++i) {
    mults(i) = int(data[at++]);
  }
  TColgp_Array1OfPnt poles(1, np);
  for (int i = 1; i <= np; ++i) {
    poles(i) = gp_Pnt(data[at], data[at + 1], data[at + 2]);
    at += 3;
  }
  if (!rational) {
    return new Geom_BSplineCurve(poles, knots, mults, degree);
  }
  TColStd_Array1OfReal weights(1, np);
  for (int i = 1; i <= np; ++i) {
    weights(i) = data[at++];
  }
  return new Geom_BSplineCurve(poles, weights, knots, mults, degree);
}

// `count` curves from the encoding, joined end to end into one wire.
static TopoDS_Wire wire_of(uint32_t count, rust::Slice<const uint32_t> spec,
                           size_t &s, rust::Slice<const double> data,
                           size_t &at) {
  BRepBuilderAPI_MakeWire w;
  for (uint32_t k = 0; k < count; ++k) {
    w.Add(BRepBuilderAPI_MakeEdge(bspline(spec, s, data, at)).Edge());
  }
  if (!w.IsDone()) {
    throw std::runtime_error("the curves don't make a wire");
  }
  return w.Wire();
}

// A loft through sections, BRepOffsetAPI_ThruSections as a solid, smooth
// (not ruled). `sizes`: the number of sections, then each section's
// number of curves; `spec` and `data` the curves in order.
std::unique_ptr<Shape> thru_sections(rust::Slice<const uint32_t> sizes,
                                     rust::Slice<const uint32_t> spec,
                                     rust::Slice<const double> data) {
  try {
    BRepOffsetAPI_ThruSections loft(Standard_True, Standard_False);
    size_t s = 0, at = 0;
    for (uint32_t k = 0; k < sizes[0]; ++k) {
      loft.AddWire(wire_of(sizes[1 + k], spec, s, data, at));
    }
    loft.Build();
    if (!loft.IsDone()) {
      throw std::runtime_error("BRepOffsetAPI_ThruSections failed");
    }
    return wrap(loft.Shape());
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("ThruSections: ") + e.GetMessageString());
  }
}

// A profile swept along a spine, BRepOffsetAPI_MakePipeShell made solid:
// `sizes` the profile's and the spine's curve counts; `binormal` empty
// for the corrected Frenet frame (OCCT's nearest to rotation-minimizing),
// else a fixed binormal direction.
std::unique_ptr<Shape> pipe_shell(rust::Slice<const uint32_t> sizes,
                                  rust::Slice<const uint32_t> spec,
                                  rust::Slice<const double> data,
                                  rust::Slice<const double> binormal) {
  try {
    size_t s = 0, at = 0;
    TopoDS_Wire profile = wire_of(sizes[0], spec, s, data, at);
    TopoDS_Wire spine = wire_of(sizes[1], spec, s, data, at);
    BRepOffsetAPI_MakePipeShell pipe(spine);
    if (binormal.empty()) {
      pipe.SetMode(Standard_False);
    } else {
      pipe.SetMode(gp_Dir(binormal[0], binormal[1], binormal[2]));
    }
    pipe.Add(profile);
    pipe.Build();
    if (!pipe.IsDone() || !pipe.MakeSolid()) {
      throw std::runtime_error("BRepOffsetAPI_MakePipeShell failed");
    }
    return wrap(pipe.Shape());
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("MakePipeShell: ") + e.GetMessageString());
  }
}

// The solid with every face moved `d` along its outward normal,
// BRepOffsetAPI_MakeOffsetShape by join with arcs at opening edges, as
// ryukei rolls a ball there.
std::unique_ptr<Shape> offset_shape(const Shape &s, double d) {
  try {
    BRepOffsetAPI_MakeOffsetShape offset;
    offset.PerformByJoin(s.shape, d, 1e-7, BRepOffset_Skin, Standard_False,
                         Standard_False, GeomAbs_Arc);
    if (!offset.IsDone()) {
      throw std::runtime_error("BRepOffsetAPI_MakeOffsetShape failed");
    }
    return wrap(offset.Shape());
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("MakeOffsetShape: ") + e.GetMessageString());
  }
}

// The face of `s` nearest (x, y, z).
static TopoDS_Face nearest_face(const TopoDS_Shape &s, double x, double y,
                                double z) {
  TopoDS_Vertex v = BRepBuilderAPI_MakeVertex(gp_Pnt(x, y, z));
  TopoDS_Face best;
  double least = 1e300;
  for (TopExp_Explorer ex(s, TopAbs_FACE); ex.More(); ex.Next()) {
    BRepExtrema_DistShapeShape d(v, ex.Current());
    if (d.IsDone() && d.Value() < least) {
      least = d.Value();
      best = TopoDS::Face(ex.Current());
    }
  }
  if (best.IsNull()) {
    throw std::runtime_error("no face");
  }
  return best;
}

// The solid hollowed to `thickness` with the faces nearest `open` (x, y,
// z flattened) removed, BRepOffsetAPI_MakeThickSolid by join, inwards
// (OCCT's offset is negative inwards).
std::unique_ptr<Shape> thick_solid(const Shape &s, rust::Slice<const double> open,
                                   double thickness) {
  try {
    TopTools_ListOfShape faces;
    for (size_t k = 0; k + 2 < open.size(); k += 3) {
      faces.Append(nearest_face(s.shape, open[k], open[k + 1], open[k + 2]));
    }
    BRepOffsetAPI_MakeThickSolid thick;
    thick.MakeThickSolidByJoin(s.shape, faces, -thickness, 1e-7, BRepOffset_Skin,
                               Standard_False, Standard_False, GeomAbs_Arc);
    if (!thick.IsDone()) {
      throw std::runtime_error("BRepOffsetAPI_MakeThickSolid failed");
    }
    return wrap(thick.Shape());
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("MakeThickSolid: ") + e.GetMessageString());
  }
}

// Where the faces of `a` and `b` nearest `pa` and `pb` meet,
// BRepAlgoAPI_Section of the two faces (OCCT's booleans' intersection;
// GeomAPI_IntSS on the bare rational surfaces gave curves off them): each
// edge's curve sampled at `samples + 1` parameters over its range, after
// the number of edges, each with its point count.
rust::Vec<double> intersect_faces(const Shape &a, rust::Slice<const double> pa,
                                  const Shape &b, rust::Slice<const double> pb,
                                  uint32_t samples) {
  try {
    TopoDS_Face fa = nearest_face(a.shape, pa[0], pa[1], pa[2]);
    TopoDS_Face fb = nearest_face(b.shape, pb[0], pb[1], pb[2]);
    BRepAlgoAPI_Section section(fa, fb, Standard_False);
    section.Approximation(Standard_True);
    section.Build();
    if (!section.IsDone() || section.HasErrors()) {
      throw std::runtime_error("BRepAlgoAPI_Section failed");
    }
    rust::Vec<double> out;
    std::vector<TopoDS_Edge> edges;
    for (TopExp_Explorer ex(section.Shape(), TopAbs_EDGE); ex.More(); ex.Next()) {
      edges.push_back(TopoDS::Edge(ex.Current()));
    }
    out.push_back(double(edges.size()));
    for (const TopoDS_Edge &e : edges) {
      BRepAdaptor_Curve c(e);
      double t0 = c.FirstParameter(), t1 = c.LastParameter();
      out.push_back(double(samples + 1));
      for (uint32_t k = 0; k <= samples; ++k) {
        gp_Pnt q = c.Value(t0 + (t1 - t0) * k / samples);
        out.push_back(q.X());
        out.push_back(q.Y());
        out.push_back(q.Z());
      }
    }
    return out;
  } catch (const Standard_Failure &e) {
    throw std::runtime_error(std::string("BRepAlgoAPI_Section: ") + e.GetMessageString());
  }
}

} // namespace ryukei_occt
