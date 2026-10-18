#include "ryukei-bench/src/occt/shim.h"
#include "ryukei-bench/src/occt.rs.h"

#include <BRepAlgoAPI_Cut.hxx>
#include <BRepBuilderAPI_MakeVertex.hxx>
#include <BRepBuilderAPI_NurbsConvert.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <BRepExtrema_DistShapeShape.hxx>
#include <BRepGProp.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCone.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakeSphere.hxx>
#include <BRepPrimAPI_MakeTorus.hxx>
#include <BRep_Builder.hxx>
#include <BRep_Tool.hxx>
#include <GProp_GProps.hxx>
#include <Poly_Triangulation.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Iterator.hxx>
#include <gp_Trsf.hxx>

#include <stdexcept>

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
  TopTools_ListOfShape arguments, list;
  arguments.Append(s.shape);
  for (TopoDS_Iterator it(tools.shape); it.More(); it.Next()) {
    list.Append(it.Value());
  }
  BRepAlgoAPI_Cut op;
  op.SetArguments(arguments);
  op.SetTools(list);
  op.SetRunParallel(parallel);
  op.Build();
  if (!op.IsDone() || op.HasErrors()) {
    throw std::runtime_error("BRepAlgoAPI_Cut failed");
  }
  return wrap(op.Shape());
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

} // namespace ryukei_occt
