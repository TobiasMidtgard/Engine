//! The camera's lens effects on the finished HDR image: depth of field, then motion blur (after
//! TAA, before bloom and tonemapping; see [`crate::depth_of_field`] and [`crate::motion_blur`]
//! for why there). Both are off unless the active camera has the component; their GPU resources
//! are created the first time a camera uses them.

use crate::depth_of_field::{DepthOfField, DofPass};
use crate::features::GpuShared;
use crate::frame::ViewData;
use crate::motion_blur::{BlurSource, MotionBlur, MotionBlurPass};
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

    /// Record the effects the camera asks for, leaving the result in `targets.hdr`. Motion blur
    /// is skipped when the previous frame is not this view's (`view.history_valid`), and depth
    /// of field for orthographic cameras.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &mut self,
        shared: &GpuShared,
        targets: &Targets,
        encoder: &mut wgpu::CommandEncoder,
        view: &ViewData,
        dof: Option<&DepthOfField>,
        motion_blur: Option<&MotionBlur>,
        real_dt: f32,
    ) {
        let dof = dof.filter(|_| !view.orthographic);
        let motion_blur = motion_blur.filter(|m| view.history_valid && m.shutter_fraction > 0.0);
        if dof.is_none()
            && let Some(pass) = &mut self.dof
        {
            pass.skip();
        }
        if dof.is_none() && motion_blur.is_none() {
            return;
        }
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
        let scratch = self.scratch.as_ref().expect("scratch target");
        // Where the image is now.
        let mut in_scratch = false;
        if let Some(settings) = dof {
            let pass = self.dof.get_or_insert_with(|| DofPass::new(shared));
            pass.encode(
                shared,
                targets,
                encoder,
                view,
                settings,
                real_dt,
                &scratch.view,
            );
            in_scratch = true;
        }
        if let Some(settings) = motion_blur {
            let pass = self
                .motion_blur
                .get_or_insert_with(|| MotionBlurPass::new(shared));
            let (source, out) = if in_scratch {
                (BlurSource::Scratch, &targets.hdr.view)
            } else {
                (BlurSource::Hdr, &scratch.view)
            };
            pass.encode(
                shared, targets, scratch, encoder, view, settings, real_dt, source, out,
            );
            in_scratch = !in_scratch;
        }
        if in_scratch {
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
