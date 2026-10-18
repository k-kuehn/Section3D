//! All user-facing settings, grouped by workflow step. Each group hashes to a
//! fingerprint so steps can tell when their results are out of date.


use serde::{Deserialize, Serialize};

use crate::printers::Printer;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrinterSettings {
    pub printer: Printer,
    /// Clearance kept from each edge of the build volume (mm).
    pub margin: f32,
}

impl Default for PrinterSettings {
    fn default() -> Self {
        PrinterSettings { printer: Printer::default(), margin: 5.0 }
    }
}

impl PrinterSettings {
    /// Usable build volume after margins (X, Y, Z).
    pub fn usable(&self) -> [f32; 3] {
        let v = self.printer.volume;
        [(v[0] - 2.0 * self.margin).max(10.0), (v[1] - 2.0 * self.margin).max(10.0), (v[2] - self.margin).max(10.0)]
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrepareSettings {
    /// Rotation about X, Y, Z in degrees (applied in that order).
    pub rotate: [f32; 3],
    /// Uniform scale in percent.
    pub scale: f32,
    pub mirror: [bool; 3],
    /// Shell wall thickness used by Hollow (mm).
    pub wall: f32,
}

impl Default for PrepareSettings {
    fn default() -> Self {
        PrepareSettings { rotate: [0.0; 3], scale: 100.0, mirror: [false; 3], wall: 3.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SplitMode {
    Triangle,
    Grid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SplitSettings {
    pub mode: SplitMode,
    /// Target piece size (mm); `None` means the largest that fits the printer.
    pub target_size: Option<f32>,
    /// Pieces smaller than this in any direction are merged away (mm).
    pub min_piece: f32,
    /// Overhang angle from vertical beyond which a surface needs support (degrees).
    pub overhang: f32,
    /// Angle of the triangle side cuts from the base (degrees).
    pub side_angle: f32,
    /// Smart search time limit (seconds).
    pub time_limit: f32,
}

impl Default for SplitSettings {
    fn default() -> Self {
        SplitSettings { mode: SplitMode::Triangle, target_size: None, min_piece: 15.0, overhang: 45.0, side_angle: 45.0, time_limit: 20.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LabelMode {
    Off,
    Emboss,
    Deboss,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectorSettings {
    pub dowels: bool,
    /// Dowel pin diameter (mm).
    pub diameter: f32,
    /// Extra hole diameter beyond the pin (mm).
    pub clearance: f32,
    /// Total pin length; each hole is half this plus clearance deep (mm).
    pub length: f32,
    /// Pins per cut face.
    pub per_cut: u32,
    pub labels: LabelMode,
    /// Label text height (mm).
    pub label_size: f32,
    /// Emboss height / deboss depth (mm).
    pub label_depth: f32,
}

impl Default for ConnectorSettings {
    fn default() -> Self {
        ConnectorSettings { dowels: true, diameter: 6.0, clearance: 0.2, length: 30.0, per_cut: 2, labels: LabelMode::Deboss, label_size: 8.0, label_depth: 0.6 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub folder: Option<String>,
    /// Tokens: {project}, {bed}, {part}.
    pub bed_name: String,
    pub part_name: String,
    pub single_stls: bool,
    pub viewer: bool,
    pub guide: bool,
    /// Gap between pieces on a bed (mm).
    pub spacing: f32,
    pub keep_assembly_order: bool,
    /// Shrinkage compensation (percent added) for X/Y and Z.
    pub shrink_xy: f32,
    pub shrink_z: f32,
    /// Filament density (g/cm³) for weight estimates.
    pub density: f32,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            folder: None,
            bed_name: "{project}_bed{bed}".into(),
            part_name: "{project}_{part}".into(),
            single_stls: false,
            viewer: true,
            guide: true,
            spacing: 4.0,
            keep_assembly_order: false,
            shrink_xy: 0.0,
            shrink_z: 0.0,
            density: 1.24,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub printer: PrinterSettings,
    pub prepare: PrepareSettings,
    pub split: SplitSettings,
    pub connectors: ConnectorSettings,
    pub export: ExportSettings,
}

/// Stable fingerprint (FNV-1a of the JSON form), saved in project files.
pub fn fingerprint<T: Serialize>(v: &T) -> u64 {
    let s = serde_json::to_vec(v).unwrap_or_default();
    s.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}
