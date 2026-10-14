# ryucad

CAD built for AI agents to drive, with a from-scratch B-rep kernel (**ryukei**) designed for large-assembly performance. The long-term target is Parasolid-level capability.

- `crates/ryukei`: geometry kernel (single bodies: geometry, topology, booleans, blends, tessellation)
- `crates/ryucad`: application layer (documents, assemblies, instancing, persistence, agent interface)
- `docs/`: design documents
