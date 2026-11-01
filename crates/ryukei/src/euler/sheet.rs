//! The sheet operators (design 295): MVSR moves a shell between the void
//! around everything and a solid region of its own; KFL and MFL kill and
//! make a face on a sheet's boundary, leaving or taking laminar edges.

use super::{finish, live, next_id};
use crate::name::Journal;
use crate::topo::{Body, Region, RegionId, RegionKind, ShellId};
use crate::{Error, TopologyError};

/// What [`mvsr`] did: the solid region the shell went into, or `None`
/// when it went out into the void.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mvsr {
    pub region: Option<RegionId>,
}

/// Move shell region: a solid region's only shell goes out into the void
/// region and the solid region goes (a solid becomes a closed sheet); a
/// closed shell in the void, one with no laminar edges, goes into a new
/// solid region (a closed sheet becomes a solid, its faces' normals its
/// outside). Its own inverse. A solid with cavities is `NotRemovable`; a
/// shell with laminar edges can't enclose anything (`OpenShell`).
///
/// Names: nothing is renamed, so the journal is empty.
pub fn mvsr(body: &mut Body, shell: ShellId) -> Result<(Mvsr, Journal), Error> {
    let region = live(&body.shells, shell)?.region;
    let outer = body.outer_region();
    if region == outer {
        let open = body.shell_faces(shell).any(|f| {
            body.face_loops(f)
                .any(|l| body.loop_fins(l).any(|x| body.fins[x].around == x))
        });
        if open {
            return Err(TopologyError::OpenShell.into());
        }
        let r = next_id(&body.regions);
        body.unlink_shell(shell);
        body.regions.push(Region {
            kind: RegionKind::Solid,
            first_shell: None,
        });
        body.link_shell(r, shell);
        finish(body);
        Ok((Mvsr { region: Some(r) }, Journal::new()))
    } else {
        if body.shells[shell].next != shell {
            return Err(TopologyError::NotRemovable.into());
        }
        body.unlink_shell(shell);
        body.regions.kill(region);
        body.link_shell(outer, shell);
        finish(body);
        Ok((Mvsr { region: None }, Journal::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{check, check_topology};
    use crate::euler::tests::K;
    use crate::geom::Rigid;
    use crate::primitive::block;
    use crate::topo::BodyKind;

    #[test]
    fn a_solid_goes_out_into_the_void_and_back() {
        let (mut b, _) = block(K, 1.0, 0.6, 0.4, &Rigid::IDENTITY).unwrap();
        let s = b.shells().next().unwrap();
        let (m, j) = mvsr(&mut b, s).unwrap();
        assert_eq!(m.region, None);
        assert!(j.is_empty());
        assert_eq!(b.kind(), BodyKind::Sheet);
        assert_eq!(b.regions().count(), 1);
        assert_eq!(check_topology(&b), []);
        let (m, _) = mvsr(&mut b, s).unwrap();
        assert!(m.region.is_some());
        assert_eq!(b.kind(), BodyKind::Solid);
        assert_eq!(check(&b), []);
    }

    #[test]
    fn a_solid_with_a_cavity_stays_one() {
        let mut b = crate::topo::fixture::hollow_cube();
        let s = b.shells().next().unwrap();
        assert_eq!(
            mvsr(&mut b, s).unwrap_err(),
            TopologyError::NotRemovable.into()
        );
        assert_eq!(b.kind(), BodyKind::Solid);
    }
}
