// A small C++ shim over SMLib (NVIDIA's USD BRep kernel, Apache-2.0) for
// the comparison benchmarks: build the same primitives ryukei builds,
// mesh them with ConvertToPolyBrep, and hand the triangles back.
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
};

std::unique_ptr<Body> make(uint32_t kind, double a, double b, double c,
                           rust::Slice<const double> at, bool nurbs);
SmlibMesh mesh(const Body &body, double chord, double angle, bool parallel);

} // namespace ryukei_smlib
