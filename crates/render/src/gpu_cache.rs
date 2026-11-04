//! Shader modules, bind group layouts, pipeline layouts and pipelines made once and handed out
//! again when asked for with the same description (`GpuShared::shader`,
//! `GpuShared::bind_group_layout`, `GpuShared::pipeline_layout`, `GpuShared::render_pipeline`,
//! `GpuShared::compute_pipeline`).
//!
//! Every view (the main one, a mirror, a portal, an inset) builds the same post-processing,
//! screen-effect, atmosphere and fog pipelines. Without the cache a new extra view compiled them
//! all again on the frame it appeared (about 90 ms on the main thread on Tobias's PC when a lake
//! mirror came into range, 850 ms under lavapipe). With it, a view after the first takes the
//! main view's. This is a pipeline state cache in the manner of Unreal's PSO cache and Unity's
//! shader variant collections: descriptions as keys, compiled objects as values.
//!
//! Keys hash every field of the description. Shader modules, layouts and bind group layouts
//! hash by identity (wgpu compares them by handle), so a pipeline is shared only when it was
//! described with the very same module and layout, which is why layouts and modules go through
//! the cache too: a view that makes its own layouts gets the main view's handles back.
//! Pipeline-overridable `constants` are hashed by name and value bits.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// See the module docs.
#[derive(Default)]
pub(crate) struct GpuCache {
    shaders: Mutex<HashMap<u64, wgpu::ShaderModule>>,
    bind_group_layouts: Mutex<HashMap<u64, wgpu::BindGroupLayout>>,
    pipeline_layouts: Mutex<HashMap<u64, wgpu::PipelineLayout>>,
    render_pipelines: Mutex<HashMap<u64, wgpu::RenderPipeline>>,
    compute_pipelines: Mutex<HashMap<u64, wgpu::ComputePipeline>>,
    /// Objects made so far (not found in the cache).
    made: std::sync::atomic::AtomicU64,
}

fn hasher() -> std::collections::hash_map::DefaultHasher {
    std::collections::hash_map::DefaultHasher::new()
}

fn hash_options(options: &wgpu::PipelineCompilationOptions, h: &mut impl Hasher) {
    options.constants.len().hash(h);
    for (name, value) in options.constants {
        name.hash(h);
        value.to_bits().hash(h);
    }
    options.zero_initialize_workgroup_memory.hash(h);
}

impl GpuCache {
    /// Shader modules, layouts and pipelines made so far: the rest were found in the cache.
    pub fn made(&self) -> u64 {
        self.made.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The value under `key`, or `make()`'s, kept. The lock isn't held while making: two threads
    /// asking at once may both make it, and the first one kept wins.
    fn get_or_make<T: Clone>(
        &self,
        map: &Mutex<HashMap<u64, T>>,
        key: u64,
        make: impl FnOnce() -> T,
    ) -> T {
        use std::sync::atomic::Ordering::Relaxed;
        if let Some(v) = map.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return v.clone();
        }
        let made = make();
        self.made.fetch_add(1, Relaxed);
        map.lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_insert(made)
            .clone()
    }

    pub fn shader(
        &self,
        label: &str,
        source: &str,
        with_frame: bool,
        make: impl FnOnce() -> wgpu::ShaderModule,
    ) -> wgpu::ShaderModule {
        let mut h = hasher();
        (label, source, with_frame).hash(&mut h);
        self.get_or_make(&self.shaders, h.finish(), make)
    }

    pub fn bind_group_layout(
        &self,
        device: &wgpu::Device,
        desc: &wgpu::BindGroupLayoutDescriptor,
    ) -> wgpu::BindGroupLayout {
        let mut h = hasher();
        (desc.label, desc.entries).hash(&mut h);
        self.get_or_make(&self.bind_group_layouts, h.finish(), || {
            device.create_bind_group_layout(desc)
        })
    }

    pub fn pipeline_layout(
        &self,
        device: &wgpu::Device,
        desc: &wgpu::PipelineLayoutDescriptor,
    ) -> wgpu::PipelineLayout {
        let mut h = hasher();
        (desc.label, desc.bind_group_layouts, desc.immediate_size).hash(&mut h);
        self.get_or_make(&self.pipeline_layouts, h.finish(), || {
            device.create_pipeline_layout(desc)
        })
    }

    pub fn render_pipeline(
        &self,
        device: &wgpu::Device,
        desc: &wgpu::RenderPipelineDescriptor,
    ) -> wgpu::RenderPipeline {
        let mut h = hasher();
        (desc.label, desc.layout).hash(&mut h);
        let v = &desc.vertex;
        (v.module, v.entry_point, v.buffers).hash(&mut h);
        hash_options(&v.compilation_options, &mut h);
        (desc.primitive, &desc.depth_stencil, desc.multisample).hash(&mut h);
        match &desc.fragment {
            Some(f) => {
                (true, f.module, f.entry_point, f.targets).hash(&mut h);
                hash_options(&f.compilation_options, &mut h);
            }
            None => false.hash(&mut h),
        }
        (desc.multiview_mask, desc.cache.is_some()).hash(&mut h);
        self.get_or_make(&self.render_pipelines, h.finish(), || {
            device.create_render_pipeline(desc)
        })
    }

    pub fn compute_pipeline(
        &self,
        device: &wgpu::Device,
        desc: &wgpu::ComputePipelineDescriptor,
    ) -> wgpu::ComputePipeline {
        let mut h = hasher();
        (desc.label, desc.layout, desc.module, desc.entry_point).hash(&mut h);
        hash_options(&desc.compilation_options, &mut h);
        desc.cache.is_some().hash(&mut h);
        self.get_or_make(&self.compute_pipelines, h.finish(), || {
            device.create_compute_pipeline(desc)
        })
    }
}
