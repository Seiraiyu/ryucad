//! `ryucad.mesh.1` and `ryucad.mesh.2` from bytes, as the app reads them
//! from a socket. Any
//! input must decode to meshes whose every index is in range, or give a
//! structured error. Any panic is a bug.

#![no_main]

use libfuzzer_sys::fuzz_target;
use ryucad::mesh::decode;

fuzz_target!(|data: &[u8]| {
    let Ok(bodies) = decode(data) else {
        return;
    };
    for b in bodies {
        let Ok(m) = b.mesh else { continue };
        let (np, nc, nt) = (m.positions.len(), m.corners.len(), m.triangles.len());
        assert!(m.corners.iter().all(|(p, _)| (*p as usize) < np));
        assert!(m.triangles.iter().flatten().all(|c| (*c as usize) < nc));
        for f in &m.faces {
            assert!(f.corners.0 <= f.corners.1 && f.corners.1 as usize <= nc);
            assert!(f.triangles.0 <= f.triangles.1 && f.triangles.1 as usize <= nt);
        }
        assert!(m.edges.iter().flat_map(|(_, ps)| ps).all(|p| (*p as usize) < np));
        assert!(m.vertices.iter().all(|(_, p)| (*p as usize) < np));
    }
});
