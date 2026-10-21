//! Cut plans (nested slabs of parallel planes), applied to a solid in parallel.

use crate::job::{Cancelled, JobCtx};
use crate::manifold::{GeomError, Solid};
use crate::plane::Plane;

#[derive(Clone, Debug)]
pub enum Plan {
    Leaf,
    /// Cut by several planes in order, each splitting the remainder on its
    /// negative side; the positive side of each goes to the matching child.
    Chain { planes: Vec<Plane>, children: Vec<Plan> },
}

impl Plan {
    /// Ordered parallel slabs: `planes` sorted along their common normal;
    /// `children.len() == planes.len() + 1`, first child is below the first plane.
    pub fn slabs(planes: Vec<Plane>, children: Vec<Plan>) -> Plan {
        assert_eq!(children.len(), planes.len() + 1);
        Plan::Chain { planes, children }
    }

    pub fn planes(&self, out: &mut Vec<Plane>) {
        match self {
            Plan::Leaf => {}
            Plan::Chain { planes, children } => {
                out.extend_from_slice(planes);
                for c in children {
                    c.planes(out);
                }
            }
        }
    }

    pub fn leaf_count(&self) -> usize {
        match self {
            Plan::Leaf => 1,
            Plan::Chain { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }
}

#[derive(Debug)]
pub enum ApplyError {
    Cancelled,
    Geometry(GeomError),
}

impl From<Cancelled> for ApplyError {
    fn from(_: Cancelled) -> Self {
        ApplyError::Cancelled
    }
}
impl From<GeomError> for ApplyError {
    fn from(e: GeomError) -> Self {
        ApplyError::Geometry(e)
    }
}

/// Applies a plan, returning one solid per leaf region (possibly with several
/// bodies; empty regions are dropped).
pub fn apply(solid: Solid, plan: &Plan, job: &JobCtx) -> Result<Vec<Solid>, ApplyError> {
    job.check()?;
    if solid.is_empty() {
        return Ok(Vec::new());
    }
    match plan {
        Plan::Leaf => Ok(vec![solid]),
        Plan::Chain { planes, children } => apply_chain(solid, planes, children, job),
    }
}

/// Divide and conquer over an ordered chain of non-crossing planes: cut at the
/// middle plane and recurse on both sides in parallel (O(n log k) instead of
/// peeling k slabs off one at a time).
fn apply_chain(solid: Solid, planes: &[Plane], children: &[Plan], job: &JobCtx) -> Result<Vec<Solid>, ApplyError> {
    job.check()?;
    if solid.is_empty() {
        return Ok(Vec::new());
    }
    if planes.is_empty() {
        return apply(solid, &children[0], job);
    }
    let m = planes.len() / 2;
    let (above, below) = solid.split_by_plane(planes[m].normal, planes[m].offset)?;
    let (rb, ra) = rayon::join(|| apply_chain(below, &planes[..m], &children[..=m], job), || apply_chain(above, &planes[m + 1..], &children[m + 1..], job));
    let mut out = rb?;
    out.extend(ra?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn slabs_and_splits() {
        let s = Solid::cube(DVec3::splat(100.0), false).unwrap();
        let rows = Plan::slabs(vec![Plane::axis(2, 30.0), Plane::axis(2, 60.0)], vec![Plan::Leaf, Plan::slabs(vec![Plane::axis(0, 50.0)], vec![Plan::Leaf, Plan::Leaf]), Plan::Leaf]);
        let out = apply(s, &rows, &JobCtx::new()).unwrap();
        assert_eq!(out.len(), 4);
        let total: f64 = out.iter().map(|s| s.volume()).sum();
        assert!((total - 1e6).abs() < 1.0);
    }
}
