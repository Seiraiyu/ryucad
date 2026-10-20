// A small C++ shim over OCCT for the comparison benchmarks: build the
// same primitives ryukei builds, sweep profiles, cut them, mesh them with
// BRepMesh, and measure.
#pragma once

#include "rust/cxx.h"
#include <TopoDS_Shape.hxx>
#include <memory>

namespace ryukei_occt {

struct OcctMesh;

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

} // namespace ryukei_occt
