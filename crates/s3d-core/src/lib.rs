//! Section3D geometry core. No UI code lives here.

pub mod bsp;
pub mod bvh;
pub mod edits;
pub mod hollow;
pub mod io;
pub mod job;
pub mod manifold;
pub mod march;
pub mod mesh;
pub mod orient;
pub mod partition;
pub mod plane;
pub mod printers;
pub mod repair;
pub mod settings;
pub mod split;
pub mod zones;

pub use glam;
