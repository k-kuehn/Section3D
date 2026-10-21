//! All user-facing settings, grouped by workflow step. Each group hashes to a
//! fingerprint so steps can tell when their results are out of date.


use serde::{Deserialize, Serialize};

/// The printer: its build volume and the margin kept free around the bed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "PrinterFile")]
pub struct PrinterSettings {
    /// Build volume X, Y, Z (mm).
    pub volume: [f32; 3],
    /// Clearance kept from each edge of the build volume (mm).
    pub margin: f32,
}

impl Default for PrinterSettings {
    fn default() -> Self {
        PrinterSettings { volume: [256.0; 3], margin: 5.0 }
    }
}

/// Stored form; older projects kept the volume in a named printer profile.
#[derive(Deserialize)]
#[serde(default)]
struct PrinterFile {
    volume: Option<[f32; 3]>,
    margin: f32,
    printer: Option<LegacyPrinter>,
}

#[derive(Deserialize)]
struct LegacyPrinter {
    volume: [f32; 3],
}

impl Default for PrinterFile {
    fn default() -> Self {
        PrinterFile { volume: None, margin: PrinterSettings::default().margin, printer: None }
    }
}

impl From<PrinterFile> for PrinterSettings {
    fn from(f: PrinterFile) -> Self {
        let volume = f.volume.or(f.printer.map(|p| p.volume)).unwrap_or(PrinterSettings::default().volume);
        PrinterSettings { volume, margin: f.margin }
    }
}

impl PrinterSettings {
    /// Usable build volume after margins (X, Y, Z).
    pub fn usable(&self) -> [f32; 3] {
        let v = self.volume;
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
    /// Broken-glass cells, then improved until every piece prints well.
    #[serde(alias = "Triangle")]
    Shatter,
    /// Evenly spaced (or user-placed) planes along X, Y and Z, not optimised.
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
    /// Narrowest a piece may get anywhere (mm).
    pub min_thickness: f32,
    /// Sharpest allowed corner between two cuts, or between a cut and the
    /// surface (degrees).
    pub min_angle: f32,
    /// Longest the split may keep improving pieces (seconds); it stops
    /// earlier once a round makes no progress.
    pub time_limit: f32,
    /// Grid mode: cut positions along X, Y and Z (mm, model coordinates);
    /// `None` spaces them evenly for the piece size.
    pub grid: Option<[Vec<f32>; 3]>,
}

impl Default for SplitSettings {
    fn default() -> Self {
        SplitSettings { mode: SplitMode::Shatter, target_size: None, min_piece: 15.0, overhang: 45.0, min_thickness: 10.0, min_angle: 25.0, time_limit: 180.0, grid: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LabelMode {
    Off,
    Emboss,
    Deboss,
}

/// Which side of a piece's skin carries its part number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LabelFace {
    /// The outward-bulging side (outside of a shell).
    Convex,
    /// The hollow side (inside of a shell, hidden once assembled).
    Concave,
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
    /// Preferred side for part numbers (pieces can be flipped one by one).
    pub label_face: LabelFace,
}

impl Default for ConnectorSettings {
    fn default() -> Self {
        ConnectorSettings { dowels: true, diameter: 6.0, clearance: 0.2, length: 30.0, per_cut: 2, labels: LabelMode::Deboss, label_size: 8.0, label_depth: 0.6, label_face: LabelFace::Concave }
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
