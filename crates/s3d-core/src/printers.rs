//! Printer profiles with published build volumes (mm).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Printer {
    pub name: String,
    /// Build volume X, Y, Z in mm.
    pub volume: [f32; 3],
}

impl Printer {
    pub fn new(name: &str, x: f32, y: f32, z: f32) -> Self {
        Printer { name: name.to_string(), volume: [x, y, z] }
    }
}

impl Default for Printer {
    fn default() -> Self {
        builtin()[0].clone()
    }
}

pub fn builtin() -> Vec<Printer> {
    vec![
        Printer::new("Generic (220 × 220 × 250)", 220.0, 220.0, 250.0),
        Printer::new("Bambu Lab X1 Carbon", 256.0, 256.0, 256.0),
        Printer::new("Bambu Lab P1S", 256.0, 256.0, 256.0),
        Printer::new("Bambu Lab P1P", 256.0, 256.0, 256.0),
        Printer::new("Bambu Lab A1", 256.0, 256.0, 256.0),
        Printer::new("Bambu Lab A1 mini", 180.0, 180.0, 180.0),
        Printer::new("Bambu Lab H2D", 325.0, 320.0, 325.0),
        Printer::new("Prusa MK4S", 250.0, 210.0, 220.0),
        Printer::new("Prusa CORE One", 250.0, 220.0, 270.0),
        Printer::new("Prusa MINI+", 180.0, 180.0, 180.0),
        Printer::new("Prusa XL", 360.0, 360.0, 360.0),
        Printer::new("Creality Ender-3 V3", 220.0, 220.0, 250.0),
        Printer::new("Creality K1C", 220.0, 220.0, 250.0),
        Printer::new("Creality K1 Max", 300.0, 300.0, 300.0),
        Printer::new("Creality K2 Plus", 350.0, 350.0, 350.0),
        Printer::new("Elegoo Neptune 4 Pro", 225.0, 225.0, 265.0),
        Printer::new("Elegoo Neptune 4 Max", 420.0, 420.0, 480.0),
        Printer::new("Anycubic Kobra 3", 250.0, 250.0, 260.0),
        Printer::new("Anycubic Kobra S1", 250.0, 250.0, 250.0),
        Printer::new("Snapmaker J1", 300.0, 200.0, 200.0),
        Printer::new("Snapmaker Artisan", 400.0, 400.0, 400.0),
        Printer::new("Qidi Q1 Pro", 245.0, 245.0, 240.0),
        Printer::new("Qidi X-Max 3", 325.0, 325.0, 315.0),
        Printer::new("Sovol SV08", 350.0, 350.0, 345.0),
        Printer::new("Voron 2.4 (350)", 350.0, 350.0, 340.0),
    ]
}
