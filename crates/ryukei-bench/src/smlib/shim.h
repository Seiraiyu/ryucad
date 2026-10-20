// A small C++ shim over SMLib (NVIDIA's USD BRep kernel, Apache-2.0) for
// the comparison benchmarks: build the same primitives ryukei builds,
// sweep profiles, cut them, mesh them with ConvertToPolyBrep, and hand
// the triangles back.
#pragma once

#include "rust/cxx.h"
#include <memory>

class SmBrep;
class SmContext;

namespace ryukei_smlib {

struct SmlibMesh;

// An SMLib body held for Rust, with the context its objects live in.
class Body {
public:
  Body();
  ~Body();
  std::unique_ptr<SmContext> context;
  SmBrep *brep = nullptr;
  // Tools waiting to be combined with `brep`.
  SmBrep *tools = nullptr;
};

std::unique_ptr<Body> make(uint32_t kind, double a, double b, double c,
                           rust::Slice<const double> at, bool nurbs);
std::unique_ptr<Body> extrude(rust::Slice<const double> frame,
                              rust::Slice<const uint32_t> sizes,
                              rust::Slice<const double> segments,
                              rust::Slice<const double> vector);
std::unique_ptr<Body> revolve(rust::Slice<const double> frame,
                              rust::Slice<const uint32_t> sizes,
                              rust::Slice<const double> segments,
                              rust::Slice<const double> axis, double angle);
void add_tool(Body &body, uint32_t kind, double a, double b, double c,
              rust::Slice<const double> at);
void subtract_tools(Body &body, bool parallel);
void combine_tools(Body &body, uint32_t op, bool parallel);
rust::Vec<double> vertices(const Body &body);
uint32_t faces(const Body &body);
SmlibMesh mesh(const Body &body, double chord, double angle, bool parallel);

} // namespace ryukei_smlib
