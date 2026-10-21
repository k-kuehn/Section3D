//! Manual edits on the current pieces. They change only the pieces they
//! touch and are not replayed: a fresh split starts over. Each applied edit
//! leaves an entry in the edit log, with the part numbers involved.

use serde::{Deserialize, Serialize};

use crate::manifold::Solid;
use crate::partition::{Partition, PieceId};
use crate::plane::Plane;

pub enum EditOp {
    Merge(Vec<PieceId>),
    /// Splits one piece with an extra cut.
    Cut(PieceId, Plane),
    Lock(PieceId, bool),
    /// Replaces pieces with bodies worked out in the background (for example
    /// two neighbours merged and divided again); `planes` are the new cuts.
    Replace { old: Vec<PieceId>, new: Vec<Solid>, planes: Vec<Plane> },
}

/// One line of the edit log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditEntry {
    pub text: String,
    /// Pieces the edit produced (or changed).
    pub pieces: Vec<PieceId>,
}

fn names(p: &Partition, ids: &[PieceId]) -> String {
    let mut l: Vec<u32> = ids.iter().filter_map(|i| p.piece(*i)).map(|q| q.label).collect();
    l.sort_unstable();
    let v: Vec<String> = l.iter().map(|x| format!("#{x}")).collect();
    match v.len() {
        0 => String::new(),
        1 => v[0].clone(),
        n => format!("{} and {}", v[..n - 1].join(", "), v[n - 1]),
    }
}

/// Numbers the pieces that replaced `old`: the first keeps the lowest old
/// number, the others get new numbers after the highest in use.
fn renumber(p: &mut Partition, old_labels: &[u32], new: &[PieceId]) {
    let mut next = p.pieces.iter().filter(|q| !new.contains(&q.id)).map(|q| q.label).max().unwrap_or(0).max(old_labels.iter().copied().max().unwrap_or(0));
    let mut keep: Vec<u32> = old_labels.to_vec();
    keep.sort_unstable();
    let mut keep = keep.into_iter();
    for id in new {
        let label = keep.next().unwrap_or_else(|| {
            next += 1;
            next
        });
        if let Some(q) = p.piece_mut(*id) {
            q.label = label;
        }
    }
}

fn replace(p: &mut Partition, old: &[PieceId], f: impl FnOnce(&mut Partition) -> Result<Vec<PieceId>, String>) -> Result<Vec<PieceId>, String> {
    if old.iter().any(|id| p.piece(*id).is_none()) {
        return Err("the piece no longer exists".into());
    }
    if old.iter().any(|id| p.piece(*id).is_some_and(|q| q.locked)) {
        return Err("a piece is locked".into());
    }
    let labels: Vec<u32> = old.iter().filter_map(|id| p.piece(*id)).map(|q| q.label).collect();
    let ids = f(p)?;
    renumber(p, &labels, &ids);
    Ok(ids)
}

/// Applies one edit. Returns the pieces whose geometry changed and the log entry.
pub fn apply(p: &mut Partition, op: EditOp) -> Result<(Vec<PieceId>, EditEntry), String> {
    match op {
        EditOp::Merge(ids) => {
            let before = names(p, &ids);
            let new = replace(p, &ids, |p| p.merge(&ids).map(|id| vec![id]).map_err(|e| e.to_string()))?;
            let text = format!("Merged {before} into {}", names(p, &new));
            Ok((new.clone(), EditEntry { text, pieces: new }))
        }
        EditOp::Cut(id, plane) => {
            let before = names(p, &[id]);
            let new = replace(p, &[id], |p| p.split_piece(id, plane).map_err(|e| e.to_string()))?;
            if new.len() < 2 {
                return Err("the cut does not divide the piece".into());
            }
            let text = format!("Cut {before} into {}", names(p, &new));
            Ok((new.clone(), EditEntry { text, pieces: new }))
        }
        EditOp::Replace { old, new, planes } => {
            let before = names(p, &old);
            let ids = replace(p, &old, |p| Ok(p.replace(&old, new, &planes)))?;
            let text = if ids.len() == old.len() { format!("Re-cut {before}") } else { format!("Re-cut {before} into {}", names(p, &ids)) };
            Ok((ids.clone(), EditEntry { text, pieces: ids }))
        }
        EditOp::Lock(id, locked) => {
            let q = p.piece_mut(id).ok_or("the piece no longer exists")?;
            q.locked = locked;
            let text = format!("{} #{}", if locked { "Locked" } else { "Unlocked" }, q.label);
            Ok((Vec::new(), EditEntry { text, pieces: vec![id] }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn edits_keep_numbers_and_log_them() {
        let mut p = Partition::from_solids(vec![Solid::cube(DVec3::splat(100.0), false).unwrap()]);
        p.split_piece(0, Plane::axis(0, 50.0)).unwrap();
        for (i, q) in p.pieces.iter_mut().enumerate() {
            q.label = i as u32 + 1;
        }
        let a = p.pieces[0].id;
        let (new, e) = apply(&mut p, EditOp::Cut(a, Plane::axis(1, 50.0))).unwrap();
        assert_eq!(new.len(), 2);
        let mut labels: Vec<u32> = p.pieces.iter().map(|q| q.label).collect();
        labels.sort_unstable();
        assert_eq!(labels, vec![1, 2, 3]);
        assert_eq!(e.text, "Cut #1 into #1 and #3");
        let (m, e) = apply(&mut p, EditOp::Merge(new)).unwrap();
        assert_eq!(p.piece(m[0]).unwrap().label, 1);
        assert_eq!(e.text, "Merged #1 and #3 into #1");
        let (_, e) = apply(&mut p, EditOp::Lock(m[0], true)).unwrap();
        assert_eq!(e.text, "Locked #1");
        assert!(apply(&mut p, EditOp::Cut(m[0], Plane::axis(2, 50.0))).is_err(), "locked pieces are not edited");
    }
}
