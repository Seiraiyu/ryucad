// A small C++ shim over OCCT for the comparison benchmarks: build the
// same primitives ryukei builds, sweep profiles, cut them, blend their
// edges, mesh them with BRepMesh, and measure.
#pragma once

#include "rust/cxx.h"
#include <TopoDS_Shape.hxx>
#include <memory>

namespace ryukei_occt {

struct OcctMesh;
struct XdeProp;

// An OCCT shape held for Rust.
class Shape {
public:
  explicit Shape(TopoDS_Shape s) : shape(std::move(s)) {}
  TopoDS_Shape shape;
};

std::unique_ptr<Shape> make_box(double dx, double dy, double dz);
std::unique_ptr<Shape> make_cylinder(double r, double h);
std::unique_ptr<Shape> make_cone(double r0, double r1, double h);
std::unique_ptr<Shape> make_sphere(double r);
std::unique_ptr<Shape> make_torus(double major, double minor);
std::unique_ptr<Shape> placed(const Shape &s, rust::Slice<const double> m);
std::unique_ptr<Shape> nurbs(const Shape &s);
std::unique_ptr<Shape> extrude(rust::Slice<const double> frame,
                               rust::Slice<const uint32_t> sizes,
                               rust::Slice<const double> segments,
                               rust::Slice<const double> vector);
std::unique_ptr<Shape> revolve(rust::Slice<const double> frame,
                               rust::Slice<const uint32_t> sizes,
                               rust::Slice<const double> segments,
                               rust::Slice<const double> axis, double angle);
std::unique_ptr<Shape> make_compound();
void add_to_compound(Shape &compound, const Shape &s);
std::unique_ptr<Shape> cut(const Shape &s, const Shape &tools, bool parallel);
std::unique_ptr<Shape> boolean(const Shape &s, const Shape &tools, uint32_t op,
                               bool parallel);
rust::Vec<double> vertices(const Shape &s);
uint32_t faces(const Shape &s);
OcctMesh mesh(const Shape &s, double chord, double angle, bool parallel);
double volume(const Shape &s);
double area(const Shape &s);
double distance(const Shape &s, double x, double y, double z);
std::unique_ptr<Shape> fillet(const Shape &s, rust::Slice<const double> edges,
                              rust::Slice<const double> radii);
std::unique_ptr<Shape> chamfer(const Shape &s, rust::Slice<const double> edges,
                               rust::Slice<const uint32_t> modes,
                               rust::Slice<const double> sizes,
                               rust::Slice<const double> faces);
std::unique_ptr<Shape> step_read(rust::Str path, bool fix);
rust::Vec<double> xde_colours(rust::Str path);
// Every label's material and named data, XDE's material mode on (OCCT
// 7.8 has no metadata mode).
rust::Vec<XdeProp> xde_properties(rust::Str path);
void step_write(const Shape &s, rust::Str path, bool ap242);
OcctMesh stl_read(rust::Str path);
rust::Vec<double> mesh_stl(const Shape &s, double chord, double angle,
                           bool parallel, rust::Str path);
rust::Vec<double> solid_props(const Shape &s);
std::unique_ptr<Shape> sew(const Shape &s, double tolerance);
std::unique_ptr<Shape> split(const Shape &s, const Shape &tools);
rust::Vec<double> face_areas(const Shape &s);
std::unique_ptr<Shape> thru_sections(rust::Slice<const uint32_t> sizes,
                                     rust::Slice<const uint32_t> spec,
                                     rust::Slice<const double> data);
std::unique_ptr<Shape> pipe_shell(rust::Slice<const uint32_t> sizes,
                                  rust::Slice<const uint32_t> spec,
                                  rust::Slice<const double> data,
                                  rust::Slice<const double> binormal);
std::unique_ptr<Shape> offset_shape(const Shape &s, double d);
std::unique_ptr<Shape> thicken_sheet(const Shape &s, double d);
std::unique_ptr<Shape> thick_solid(const Shape &s, rust::Slice<const double> open,
                                   double thickness);
std::unique_ptr<Shape> draft_angle(const Shape &s, rust::Slice<const double> faces,
                                   bool cylinders, rust::Slice<const double> pull,
                                   double angle, rust::Slice<const double> neutral,
                                   double &seconds);
std::unique_ptr<Shape> defeature(const Shape &s, rust::Slice<const double> faces,
                                 uint32_t select, double &seconds);
std::unique_ptr<Shape> offset_on_faces(const Shape &s, rust::Slice<const double> faces,
                                       rust::Slice<const double> distances);
rust::Vec<double> face_normal(const Shape &s, double x, double y, double z);
rust::Vec<double> filling(const Shape &s, rust::Slice<const double> tangent,
                          rust::Slice<const uint32_t> spec,
                          rust::Slice<const double> data, uint32_t count,
                          double scale);
rust::Vec<double> intersect_faces(const Shape &a, rust::Slice<const double> pa,
                                  const Shape &b, rust::Slice<const double> pb,
                                  uint32_t samples);

// OCCT's exact hidden-line removal of `s` seen along -towards with `x`
// to the right (view: towards then x, six numbers): the lengths of the
// visible sharp, smooth (G1) and outline lines, then the hidden ones,
// then the seconds HLRBRep_Algo and HLRBRep_HLRToShape took.
rust::Vec<double> hlr(const Shape &s, rust::Slice<const double> view);

} // namespace ryukei_occt
