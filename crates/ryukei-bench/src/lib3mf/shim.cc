#include "ryukei-bench/src/lib3mf/shim.h"
#include "ryukei-bench/src/lib3mf.rs.h"

#include <lib3mf_implicit.hpp>

#include <array>
#include <stdexcept>
#include <string>

namespace ryukei_lib3mf {

using namespace Lib3MF;

namespace {

using Matrix = std::array<std::array<double, 3>, 4>;

Matrix of(const sTransform &t) {
  Matrix m{};
  for (int r = 0; r < 4; ++r)
    for (int c = 0; c < 3; ++c)
      m[r][c] = t.m_Fields[r][c];
  return m;
}

Matrix identity() {
  Matrix m{};
  m[0][0] = m[1][1] = m[2][2] = 1.0;
  return m;
}

// `first` then `then`, in 3MF's row-vector convention: p * first * then.
Matrix compose(const Matrix &first, const Matrix &then) {
  Matrix m{};
  for (int r = 0; r < 4; ++r) {
    for (int c = 0; c < 3; ++c) {
      double v = r == 3 ? then[3][c] : 0.0;
      for (int k = 0; k < 3; ++k)
        v += first[r][k] * then[k][c];
      m[r][c] = v;
    }
  }
  return m;
}

std::array<double, 3> placed(const Matrix &m, const sPosition &p) {
  std::array<double, 3> out{};
  for (int c = 0; c < 3; ++c)
    out[c] = p.m_Coordinates[0] * m[0][c] + p.m_Coordinates[1] * m[1][c] +
             p.m_Coordinates[2] * m[2][c] + m[3][c];
  return out;
}

// The volume `object` encloses placed by `m`, adding its meshes'
// triangles to `triangles`.
double volume(PModel &model, PObject object, const Matrix &m,
              uint64_t &triangles) {
  if (object->IsMeshObject()) {
    PMeshObject mesh = model->GetMeshObjectByID(object->GetResourceID());
    std::vector<sPosition> vertices;
    std::vector<sTriangle> tris;
    mesh->GetVertices(vertices);
    mesh->GetTriangleIndices(tris);
    triangles += tris.size();
    double v = 0.0;
    for (const sTriangle &t : tris) {
      auto a = placed(m, vertices[t.m_Indices[0]]);
      auto b = placed(m, vertices[t.m_Indices[1]]);
      auto c = placed(m, vertices[t.m_Indices[2]]);
      v += (a[0] * (b[1] * c[2] - b[2] * c[1]) -
            a[1] * (b[0] * c[2] - b[2] * c[0]) +
            a[2] * (b[0] * c[1] - b[1] * c[0])) /
           6.0;
    }
    return v;
  }
  PComponentsObject comps =
      model->GetComponentsObjectByID(object->GetResourceID());
  double v = 0.0;
  for (Lib3MF_uint32 i = 0; i < comps->GetComponentCount(); ++i) {
    PComponent c = comps->GetComponent(i);
    Matrix t = c->HasTransform() ? of(c->GetTransform()) : identity();
    v += volume(model, c->GetObjectResource(), compose(t, m), triangles);
  }
  return v;
}

} // namespace

Read3mf read_3mf(rust::Str path) {
  try {
    PWrapper wrapper = CWrapper::loadLibrary();
    PModel model = wrapper->CreateModel();
    PReader reader = model->QueryReader("3mf");
    reader->SetStrictModeActive(true);
    reader->ReadFromFile(std::string(path));
    Read3mf out{};
    for (Lib3MF_uint32 i = 0; i < reader->GetWarningCount(); ++i) {
      Lib3MF_uint32 code = 0;
      out.warnings.push_back(rust::String(reader->GetWarning(i, code) + " (" +
                                          std::to_string(code) + ")"));
    }
    switch (model->GetUnit()) {
    case eModelUnit::MilliMeter: out.unit = rust::String("millimeter"); break;
    case eModelUnit::CentiMeter: out.unit = rust::String("centimeter"); break;
    case eModelUnit::Meter: out.unit = rust::String("meter"); break;
    default: out.unit = rust::String("other"); break;
    }
    PMetaDataGroup meta = model->GetMetaDataGroup();
    for (Lib3MF_uint32 i = 0; i < meta->GetMetaDataCount(); ++i) {
      PMetaData d = meta->GetMetaData(i);
      std::string ns = d->GetNameSpace();
      out.metadata.push_back(rust::String((ns.empty() ? "" : ns + ":") +
                                          d->GetName() + "=" + d->GetValue()));
    }
    PObjectIterator objects = model->GetObjects();
    while (objects->MoveNext()) {
      PObject o = objects->GetCurrentObject();
      out.objects.push_back(rust::String(o->GetName() + "|" + o->GetPartNumber()));
    }
    PBuildItemIterator items = model->GetBuildItems();
    while (items->MoveNext()) {
      PBuildItem item = items->GetCurrent();
      Matrix t = item->HasObjectTransform() ? of(item->GetObjectTransform())
                                            : identity();
      out.volume += volume(model, item->GetObjectResource(), t, out.triangles);
      out.items += 1;
    }
    return out;
  } catch (const ELib3MFException &e) {
    throw std::runtime_error(std::string("lib3mf: ") + e.what());
  }
}

} // namespace ryukei_lib3mf
