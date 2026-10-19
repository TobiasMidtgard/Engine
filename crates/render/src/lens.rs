//! The camera's lens effects on the HDR image: depth of field on the scene before TAA, motion
//! blur on the anti-aliased image after it, both before bloom and tonemapping (see
//! [`crate::depth_of_field`] and [`crate::motion_blur`] for why there). Both are off unless the
//! active camera has the component; their GPU resources are created the first time a camera
//! uses them.

use crate::depth_of_field::{DepthOfField, DofPass};
use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::motion_blur::{MotionBlur, MotionBlurPass};
use crate::targets::{Targets, Tex};

#[derive(Default)]
pub(crate) struct LensEffects {
    dof: Option<DofPass>,
    motion_blur: Option<MotionBlurPass>,
    /// The second full-res HDR image the passes ping-pong with `targets.hdr`.
    scratch: Option<Tex>,
}

impl LensEffects {
    /// The output size changed.
    pub fn resize(&mut self) {
        self.scratch = None;
        if let Some(d) = &mut self.dof {
            d.resize();
        }
        if let Some(m) = &mut self.motion_blur {
            m.resize();
        }
    }

    fn scratch(&mut self, shared: &GpuShared, targets: &Targets) -> &Tex {
        if self
            .scratch
            .as_ref()
            .is_none_or(|s| (s.width, s.height) != (targets.width, targets.height))
        {
            use wgpu::TextureUsages as U;
            self.scratch = Some(Tex::new(
                &shared.device,
                "lens effects",
                targets.width,
                targets.height,
                shared.formats.hdr,
                U::RENDER_ATTACHMENT | U::TEXTURE_BINDING | U::COPY_SRC,
            ));
        }
        self.scratch.as_ref().expect("scratch target")
    }

    /// Record depth of field if the camera asks for it (not for orthographic cameras), leaving
    /// the result in `targets.hdr`. Runs on the scene before TAA.
    pub fn encode_depth_of_field(
        &mut self,
        shared: &GpuShared,
        targets: &Targets,
        encoder: &mut wgpu::CommandEncoder,
        view: &ViewData,
        dof: Option<&DepthOfField>,
        real_dt: f32,
    ) {
        let Some(settings) = dof.filter(|_| !view.orthographic) else {
            if let Some(pass) = &mut self.dof {
                pass.skip();
            }
            return;
        };
        let scratch = self.scratch(shared, targets).view.clone();
        let pass = self.dof.get_or_insert_with(|| DofPass::new(shared));
        pass.encode(shared, targets, encoder, view, settings, real_dt, &scratch);
        self.copy_back(encoder, targets);
    }

    /// Record motion blur if the camera asks for it, leaving the result in `targets.hdr`.
    /// Skipped when the previous frame is not this view's (`view.history_valid`). Runs on the
    /// anti-aliased image after TAA.
    pub fn encode_motion_blur(
        &mut self,
        shared: &GpuShared,
        targets: &Targets,
        encoder: &mut wgpu::CommandEncoder,
        view: &ViewData,
        motion_blur: Option<&MotionBlur>,
        real_dt: f32,
    ) {
        let Some(settings) = motion_blur.filter(|m| view.history_valid && m.shutter_fraction > 0.0)
        else {
            return;
        };
        let scratch = self.scratch(shared, targets).view.clone();
        let pass = self
            .motion_blur
            .get_or_insert_with(|| MotionBlurPass::new(shared));
        pass.encode(shared, targets, encoder, view, settings, real_dt, &scratch);
        self.copy_back(encoder, targets);
    }

    fn copy_back(&self, encoder: &mut wgpu::CommandEncoder, targets: &Targets) {
        let scratch = self.scratch.as_ref().expect("scratch target");
        encoder.copy_texture_to_texture(
            scratch.texture.as_image_copy(),
            targets.hdr.texture.as_image_copy(),
            wgpu::Extent3d {
                width: targets.width,
                height: targets.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Call right after `queue.submit`.
    pub fn after_submit(&mut self) {
        if let Some(d) = &mut self.dof {
            d.after_submit();
        }
    }

    /// Collect GPU readbacks (non-blocking; call after the device was polled).
    pub fn collect(&mut self) {
        if let Some(d) = &mut self.dof {
            d.collect();
        }
    }

    /// The depth of field's focus distance (m) as last read back from the GPU.
    pub fn focus_distance_m(&self) -> Option<f32> {
        self.dof.as_ref().and_then(|d| d.focus_distance_m)
    }
}
