// A small C++ shim over lib3mf (the 3MF Consortium's reference library,
// BSD-2-Clause) for ryuformats' references: read a 3MF file strictly and
// report what it holds.
#pragma once

#include "rust/cxx.h"

namespace ryukei_lib3mf {

struct Read3mf;

// The file at `path` read by lib3mf in strict mode: its warnings, unit,
// metadata, objects and the volume its build encloses.
Read3mf read_3mf(rust::Str path);

} // namespace ryukei_lib3mf
