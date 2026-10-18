//! Manual edits, kept as a list and replayed after every split.
//!
//! Edits refer to pieces and seams by id plus a geometric fingerprint, so they
//! still find their target when a re-split renumbers pieces.

use glam::{DVec3, Vec3};
use serde::{Deserialize, Serialize};

use crate::partition::{Partition, PieceId, SeamId};
use crate::plane::Plane;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PieceRef {
    pub id: PieceId,
    pub centroid: Vec3,
    pub volume: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeamRef {
    pub id: SeamId,
    pub center: Vec3,
    pub normal: DVec3,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EditOp {
    Merge { pieces: Vec<PieceRef> },
    /// Splits one piece with an extra cut (re-split / add cut / fix support).
    Cut { piece: PieceRef, plane: Plane },
    /// Moves and/or tilts one seam; only its two pieces change.
    MoveSeam { seam: SeamRef, plane: Plane },
    LockPiece { piece: PieceRef, locked: bool },
    LockSeam { seam: SeamRef, locked: bool },
}

impl EditOp {
    pub fn describe(&self) -> String {
        match self {
            EditOp::Merge { pieces } => format!("Merge {} pieces", pieces.len()),
            EditOp::Cut { .. } => "Add cut".into(),
            EditOp::MoveSeam { .. } => "Adjust cut".into(),
            EditOp::LockPiece { locked, .. } => (if *locked { "Lock piece" } else { "Unlock piece" }).into(),
            EditOp::LockSeam { locked, .. } => (if *locked { "Lock cut" } else { "Unlock cut" }).into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EditStatus {
    Applied,
    /// Target not found after re-splitting.
    Orphaned,
    /// Target is locked or the geometry operation failed.
    Failed(String),
}

impl PieceRef {
    pub fn of(p: &Partition, id: PieceId) -> Option<PieceRef> {
        p.piece(id).map(|q| PieceRef { id, centroid: q.centroid, volume: q.volume })
    }

    /// Finds the piece: same id with a matching centroid, else nearest centroid.
    pub fn resolve(&self, p: &Partition) -> Option<PieceId> {
        let tol = (self.volume.max(1.0).cbrt() * 0.25) as f32;
        if let Some(q) = p.piece(self.id) {
            if q.centroid.distance(self.centroid) < tol {
                return Some(q.id);
            }
        }
        p.pieces
            .iter()
            .map(|q| (q.centroid.distance(self.centroid), q))
            .filter(|(d, q)| *d < tol && (q.volume / self.volume.max(1e-9) - 1.0).abs() < 0.35)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, q)| q.id)
    }
}

impl SeamRef {
    pub fn of(p: &Partition, id: SeamId) -> Option<SeamRef> {
        p.seam(id).map(|s| SeamRef { id, center: s.center, normal: s.plane.normal })
    }

    pub fn resolve(&self, p: &Partition) -> Option<SeamId> {
        let ok = |s: &crate::partition::Seam| s.plane.normal.dot(self.normal).abs() > 0.995;
        if let Some(s) = p.seam(self.id) {
            if ok(s) && s.center.distance(self.center) < 5.0 {
                return Some(s.id);
            }
        }
        p.seams
            .iter()
            .filter(|s| ok(s))
            .map(|s| (s.center.distance(self.center), s))
            .filter(|(d, _)| *d < 30.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, s)| s.id)
    }
}

/// Applies one edit. Returns the ids of pieces whose geometry changed.
pub fn apply(p: &mut Partition, op: &EditOp) -> (EditStatus, Vec<PieceId>) {
    let locked_piece = |p: &Partition, id: PieceId| p.piece(id).is_some_and(|q| q.locked);
    match op {
        EditOp::Merge { pieces } => {
            let ids: Option<Vec<PieceId>> = pieces.iter().map(|r| r.resolve(p)).collect();
            let Some(ids) = ids else { return (EditStatus::Orphaned, vec![]) };
            if ids.iter().any(|i| locked_piece(p, *i)) {
                return (EditStatus::Failed("a piece is locked".into()), vec![]);
            }
            match p.merge(&ids) {
                Ok(id) => (EditStatus::Applied, vec![id]),
                Err(e) => (EditStatus::Failed(e.to_string()), vec![]),
            }
        }
        EditOp::Cut { piece, plane } => {
            let Some(id) = piece.resolve(p) else { return (EditStatus::Orphaned, vec![]) };
            if locked_piece(p, id) {
                return (EditStatus::Failed("piece is locked".into()), vec![]);
            }
            match p.split_piece(id, *plane) {
                Ok(ids) => (EditStatus::Applied, ids),
                Err(e) => (EditStatus::Failed(e.to_string()), vec![]),
            }
        }
        EditOp::MoveSeam { seam, plane } => {
            let Some(id) = seam.resolve(p) else { return (EditStatus::Orphaned, vec![]) };
            let s = p.seam(id).unwrap();
            if s.locked || locked_piece(p, s.a) || locked_piece(p, s.b) {
                return (EditStatus::Failed("cut is locked".into()), vec![]);
            }
            match p.recut_seam(id, *plane) {
                Ok(ids) => (EditStatus::Applied, ids),
                Err(e) => (EditStatus::Failed(e.to_string()), vec![]),
            }
        }
        EditOp::LockPiece { piece, locked } => {
            let Some(id) = piece.resolve(p) else { return (EditStatus::Orphaned, vec![]) };
            p.piece_mut(id).unwrap().locked = *locked;
            (EditStatus::Applied, vec![])
        }
        EditOp::LockSeam { seam, locked } => {
            let Some(id) = seam.resolve(p) else { return (EditStatus::Orphaned, vec![]) };
            p.seams.iter_mut().find(|s| s.id == id).unwrap().locked = *locked;
            (EditStatus::Applied, vec![])
        }
    }
}

/// Replays all edits in order.
pub fn replay(p: &mut Partition, edits: &[EditOp]) -> (Vec<EditStatus>, Vec<PieceId>) {
    let mut changed = Vec::new();
    let st = edits
        .iter()
        .map(|e| {
            let (s, c) = apply(p, e);
            changed.extend(c);
            s
        })
        .collect();
    changed.retain(|id| p.piece(*id).is_some());
    changed.sort_unstable();
    changed.dedup();
    (st, changed)
}
