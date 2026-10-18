//! Section3D geometry core. No UI code lives here.

pub mod assembly;
pub mod bsp;
pub mod bvh;
pub mod connectors;
pub mod edits;
pub mod export;
pub mod hollow;
pub mod io;
pub mod job;
pub mod manifold;
pub mod march;
pub mod mesh;
pub mod orient;
pub mod pack;
pub mod partition;
pub mod plane;
pub mod printers;
pub mod project;
pub mod repair;
pub mod settings;
pub mod split;
pub mod zones;

pub use glam;
