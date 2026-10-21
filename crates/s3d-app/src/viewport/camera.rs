//! Orbit camera, Z up.

use glam::{Mat4, Vec2, Vec3};
use s3d_core::mesh::Aabb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewPreset {
    Iso,
    Top,
    Front,
    Side,
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub target: Vec3,
    /// Rotation about Z (radians).
    pub yaw: f32,
    /// Elevation above the XY plane (radians).
    pub pitch: f32,
    pub distance: f32,
    pub fov_y: f32,
    /// Scene radius, used for clip planes.
    pub scene_radius: f32,
    /// Parallel (isometric-style) projection instead of perspective. The view
    /// then shows the same extent a perspective view would at `distance`.
    pub ortho: bool,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { target: Vec3::ZERO, yaw: -0.8, pitch: 0.5, distance: 500.0, fov_y: 35f32.to_radians(), scene_radius: 200.0, ortho: false }
    }
}

impl Camera {
    /// Direction from the target towards the eye.
    fn back(&self) -> Vec3 {
        Vec3::new(self.pitch.cos() * self.yaw.cos(), self.pitch.cos() * self.yaw.sin(), self.pitch.sin())
    }

    /// Distance of the actual eye: with a parallel projection it stands back
    /// beyond the scene so nothing is clipped, whatever the zoom.
    fn eye_distance(&self) -> f32 {
        if self.ortho { self.distance.max(self.scene_radius * 3.0 + 10.0) } else { self.distance }
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.back() * self.eye_distance()
    }

    pub fn view(&self) -> Mat4 {
        let up = if self.pitch.abs() > 1.55 { Vec3::new(-self.yaw.cos(), -self.yaw.sin(), 0.0) * self.pitch.signum() } else { Vec3::Z };
        glam::camera::rh::view::look_at_mat4(self.eye(), self.target, up)
    }

    /// Near and far clip distances.
    fn clip(&self) -> (f32, f32) {
        let d = self.eye_distance();
        let far = d + self.scene_radius * 4.0;
        let near = if self.ortho { (d - self.scene_radius * 3.0).max(0.5) } else { (d - self.scene_radius * 2.0).max(far * 2e-3).max(0.5) };
        (near, far)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        let (near, far) = self.clip();
        if self.ortho {
            let h = self.distance * (self.fov_y * 0.5).tan();
            let w = h * aspect.max(1e-3);
            return glam::camera::rh::proj::directx::orthographic(-w, w, -h, h, near, far);
        }
        glam::camera::rh::proj::directx::perspective(self.fov_y, aspect.max(1e-3), near, far)
    }

    /// How far lines are pulled towards the eye so lines lying on a surface
    /// are not hidden by it: a few depth-buffer steps at their distance,
    /// well under a wall's thickness.
    pub fn line_bias(&self) -> [f32; 2] {
        let (near, far) = self.clip();
        if self.ortho {
            // Depth is linear: the same small step everywhere.
            [0.05 + (far - near) * 2e-6, 0.0]
        } else {
            // Depth precision falls off with distance² / near.
            [0.05, (far * 1e-6 / near).min(1e-3)]
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw -= delta.x * 0.008;
        self.pitch = (self.pitch + delta.y * 0.008).clamp(-1.56, 1.56);
    }

    /// Pans by a screen-space delta (pixels) given the viewport height.
    pub fn pan(&mut self, delta: Vec2, viewport_h: f32) {
        let world_per_px = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_h.max(1.0);
        let v = self.view();
        let right = Vec3::new(v.x_axis.x, v.y_axis.x, v.z_axis.x);
        let up = Vec3::new(v.x_axis.y, v.y_axis.y, v.z_axis.y);
        self.target += (-right * delta.x + up * delta.y) * world_per_px;
    }

    /// Zooms towards `toward` (a world point under the cursor), if given.
    pub fn zoom(&mut self, factor: f32, toward: Option<Vec3>) {
        let new_d = (self.distance * factor).clamp(1.0, self.scene_radius * 50.0 + 10.0);
        if let Some(p) = toward {
            let k = 1.0 - new_d / self.distance;
            self.target += (p - self.target) * k;
        }
        self.distance = new_d;
    }

    pub fn preset(&mut self, v: ViewPreset) {
        let (yaw, pitch) = match v {
            ViewPreset::Iso => (-0.8, 0.5),
            ViewPreset::Top => (-std::f32::consts::FRAC_PI_2, 1.5599),
            ViewPreset::Front => (-std::f32::consts::FRAC_PI_2, 0.0),
            ViewPreset::Side => (0.0, 0.0),
        };
        self.yaw = yaw;
        self.pitch = pitch;
    }

    pub fn fit(&mut self, bb: &Aabb, aspect: f32) {
        if bb.is_empty() {
            return;
        }
        let r = (bb.diagonal() * 0.5).max(1.0);
        self.scene_radius = r;
        self.target = bb.center();
        let horizontal = ((self.fov_y * 0.5).tan() * aspect.max(0.2)).atan();
        let half = (self.fov_y * 0.5).min(horizontal);
        self.distance = r / half.sin() * 1.08;
    }

    /// World-space ray through a point in normalised device coords (-1..1).
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj(aspect).inverse();
        let a = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.0));
        let b = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
        (a, (b - a).normalize())
    }

    /// Projects a world point to normalised device coords; None if behind.
    pub fn project(&self, p: Vec3, aspect: f32) -> Option<Vec2> {
        let c = self.view_proj(aspect) * p.extend(1.0);
        if c.w <= 1e-6 {
            return None;
        }
        Some(Vec2::new(c.x / c.w, c.y / c.w))
    }
}
