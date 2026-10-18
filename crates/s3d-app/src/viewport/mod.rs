//! 3D viewport widget: camera interaction plus the wgpu scene callback.

pub mod camera;
pub mod renderer;

use std::sync::Arc;

use egui::{Rect, Sense, Ui};
use glam::{Vec2, Vec3};

pub use camera::{Camera, ViewPreset};
pub use renderer::{DrawItem, LineSeg, MeshRef, SceneFrame, TriVert};

/// What the user did in the viewport this frame.
#[derive(Default)]
pub struct ViewportInput {
    /// Primary click (not a drag): world ray.
    pub click: Option<(Vec3, Vec3)>,
    pub right_click: Option<(Vec3, Vec3)>,
    /// Ray under the hovering pointer.
    pub hover: Option<(Vec3, Vec3)>,
    /// Primary drag while a tool owns the drag (camera does not orbit).
    pub tool_drag: Option<ToolDrag>,
    pub rect: Rect,
    pub response: Option<egui::Response>,
}

#[derive(Clone, Copy, Debug)]
pub struct ToolDrag {
    pub ray: (Vec3, Vec3),
    pub delta_px: Vec2,
    pub started: bool,
    pub stopped: bool,
}

pub struct Viewport {
    pub camera: Camera,
    pub pending_fit: bool,
    aspect: f32,
    last_size: Vec2,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport { camera: Camera::default(), pending_fit: true, aspect: 1.5, last_size: Vec2::new(800.0, 600.0) }
    }
}

impl Viewport {
    pub fn aspect(&self) -> f32 {
        self.aspect
    }

    pub fn size(&self) -> Vec2 {
        self.last_size
    }

    /// Ray through a screen position inside `rect`.
    pub fn ray_at(&self, rect: Rect, pos: egui::Pos2) -> (Vec3, Vec3) {
        let ndc = Vec2::new((pos.x - rect.left()) / rect.width() * 2.0 - 1.0, 1.0 - (pos.y - rect.top()) / rect.height() * 2.0);
        self.camera.ray(ndc, self.aspect)
    }

    /// Screen position of a world point.
    pub fn to_screen(&self, rect: Rect, p: Vec3) -> Option<egui::Pos2> {
        let n = self.camera.project(p, self.aspect)?;
        Some(egui::pos2(rect.left() + (n.x + 1.0) * 0.5 * rect.width(), rect.top() + (1.0 - n.y) * 0.5 * rect.height()))
    }

    /// Allocates the viewport, handles camera input and returns user input.
    /// `tool_owns_drag` makes primary drags go to the active tool.
    /// `pick_depth` resolves the world point under a ray for zoom-to-cursor.
    pub fn interact(&mut self, ui: &mut Ui, tool_owns_drag: bool, pick_depth: &dyn Fn((Vec3, Vec3)) -> Option<Vec3>) -> ViewportInput {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        self.aspect = rect.width() / rect.height().max(1.0);
        self.last_size = Vec2::new(rect.width(), rect.height());
        let mut input = ViewportInput { rect, ..Default::default() };

        let (primary_drag, secondary_drag, middle_drag) = ui.input(|i| (i.pointer.primary_down(), i.pointer.secondary_down(), i.pointer.middle_down()));
        let shift = ui.input(|i| i.modifiers.shift);
        if resp.dragged() {
            let d = resp.drag_delta();
            let d = Vec2::new(d.x, d.y);
            if middle_drag || secondary_drag || (primary_drag && shift && !tool_owns_drag) {
                self.camera.pan(d, rect.height());
            } else if primary_drag {
                if tool_owns_drag {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        input.tool_drag = Some(ToolDrag { ray: self.ray_at(rect, pos), delta_px: d, started: resp.drag_started(), stopped: false });
                    }
                } else {
                    self.camera.orbit(d);
                }
            }
        }
        if resp.drag_stopped() && tool_owns_drag {
            if let Some(pos) = resp.interact_pointer_pos().or(resp.hover_pos()) {
                input.tool_drag = Some(ToolDrag { ray: self.ray_at(rect, pos), delta_px: Vec2::ZERO, started: false, stopped: true });
            }
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y + i.zoom_delta().ln() * 300.0);
            if scroll.abs() > 0.0 {
                let toward = resp.hover_pos().and_then(|p| pick_depth(self.ray_at(rect, p)));
                self.camera.zoom((-scroll * 0.0015).exp(), toward);
            }
        }
        if let Some(pos) = resp.hover_pos() {
            input.hover = Some(self.ray_at(rect, pos));
        }
        if resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                input.click = Some(self.ray_at(rect, pos));
            }
        }
        if resp.secondary_clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                input.right_click = Some(self.ray_at(rect, pos));
            }
        }
        input.response = Some(resp);
        input
    }

    /// Queues the scene for drawing into `rect`.
    pub fn paint(&self, ui: &Ui, rect: Rect, mut frame: SceneFrame) {
        let ppp = ui.ctx().pixels_per_point();
        frame.size_px = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
        frame.view_proj = self.camera.view_proj(self.aspect);
        frame.eye = self.camera.eye();
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, renderer::ViewportCallback { frame: Arc::new(frame) }));
    }
}
