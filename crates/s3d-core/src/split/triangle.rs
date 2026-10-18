//! Triangle mode: brick-staggered triangular pieces that print without supports.

use crate::bsp::Plan;
use crate::job::JobCtx;
use crate::mesh::Mesh;
use crate::settings::Settings;
use crate::zones::Zones;

pub fn plan(mesh: &Mesh, s: &Settings, zones: &Zones, _job: &JobCtx) -> Plan {
    let bed = s.printer.usable();
    let cell = |a: usize| s.split.target_size.map_or(bed[a], |t| t.min(bed[a]));
    super::grid::plan(&mesh.bbox(), [cell(0), cell(1), cell(2)], zones)
}
