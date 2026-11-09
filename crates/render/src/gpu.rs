//! GPU device creation for windowed and headless rendering.
//!
//! Backend selection: `WGPU_BACKEND=vulkan|metal|dx12|gl` overrides the default (all native
//! backends, best adapter first). Headless machines without a GPU can install a software
//! Vulkan driver (lavapipe: `apt install mesa-vulkan-drivers`) and render screenshots.
//! `WGPU_BACKEND=noop` gets a device that executes nothing, to time the CPU side alone.

use std::fmt;
use std::sync::Arc;

use engine_core::window::WindowHandleSource;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, WindowHandle,
};

/// Lets wgpu keep the platform's window alive for as long as the surface exists.
#[derive(Clone)]
pub(crate) struct WindowTarget(pub Arc<dyn WindowHandleSource>);

impl HasWindowHandle for WindowTarget {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        self.0.window_handle()
    }
}

impl HasDisplayHandle for WindowTarget {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        self.0.display_handle()
    }
}

impl fmt::Debug for WindowTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WindowTarget")
    }
}

/// The device everything renders with.
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
}

/// The window's swapchain.
pub struct SurfaceState {
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    /// sRGB view format used when blitting to the surface.
    pub view_format: wgpu::TextureFormat,
}

impl SurfaceState {
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(device, &self.config);
    }

    pub fn set_vsync(&mut self, device: &wgpu::Device, vsync: bool) {
        let mode = if vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        if self.config.present_mode != mode {
            self.config.present_mode = mode;
            self.surface.configure(device, &self.config);
        }
    }
}

/// Create a device, and a surface when a window is given.
pub fn create(
    window: Option<Arc<dyn WindowHandleSource>>,
    width: u32,
    height: u32,
    vsync: bool,
) -> Result<(Gpu, Option<SurfaceState>), String> {
    // Native APIs first; OpenGL only as a fallback (or when WGPU_BACKEND asks for it).
    let attempts = match wgpu::Backends::from_env() {
        Some(b) => vec![b],
        None => vec![wgpu::Backends::PRIMARY, wgpu::Backends::SECONDARY],
    };
    let mut last_error = String::from("no backends");
    for backends in attempts {
        match try_create(window.clone(), backends, width, height, vsync) {
            Ok(r) => return Ok(r),
            Err(e) => {
                tracing::debug!("{backends:?}: {e}");
                last_error = e;
            }
        }
    }
    Err(last_error)
}

/// The noop device stands in for a discrete GPU with the limits of the machine's real (or
/// software) Vulkan adapter: its default, maximally permissive limits overflow wgpu's own
/// fixed-size tables.
fn noop_options() -> wgpu::NoopBackendOptions {
    let mut real = wgpu::InstanceDescriptor::new_without_display_handle();
    real.backends = wgpu::Backends::PRIMARY;
    let limits = pollster::block_on(
        wgpu::Instance::new(real).request_adapter(&wgpu::RequestAdapterOptions::default()),
    )
    .map_or_else(|_| wgpu::Limits::default(), |a| a.limits());
    let mut options = wgpu::NoopBackendOptions::enabled();
    options.limits = Some(limits);
    options.device_type = Some(wgpu::DeviceType::DiscreteGpu);
    options
}

/// How the GPU allocator takes memory from the driver: in blocks of 16 to 64 MB that it hands
/// out pieces of, with anything larger in an allocation of its own. wgpu's "performance" preset
/// takes 128 to 256 MB blocks, and a block only goes back to the driver once everything in it is
/// freed; with thousands of textures and buffers of a few MB each coming and going (zones
/// streaming, images recompressed), gigabytes stayed reserved in mostly empty blocks, which is
/// what the driver, Task Manager and the other programs on the card see. Smaller blocks empty
/// far more often. `ENGINE_GPU_BLOCK_MB=<n>` sets the largest block (256 = the old preset), to
/// compare on one build.
fn memory_hints() -> wgpu::MemoryHints {
    const MB: u64 = 1 << 20;
    let largest = std::env::var("ENGINE_GPU_BLOCK_MB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|n| (4..=256).contains(n));
    match largest {
        Some(256) => wgpu::MemoryHints::Performance,
        Some(n) => wgpu::MemoryHints::Manual {
            suballocated_device_memory_block_size: (n / 4).max(4) * MB..n * MB,
        },
        None => wgpu::MemoryHints::Manual {
            suballocated_device_memory_block_size: 16 * MB..64 * MB,
        },
    }
}

fn try_create(
    window: Option<Arc<dyn WindowHandleSource>>,
    backends: wgpu::Backends,
    width: u32,
    height: u32,
    vsync: bool,
) -> Result<(Gpu, Option<SurfaceState>), String> {
    let mut desc = match &window {
        Some(w) => {
            wgpu::InstanceDescriptor::new_with_display_handle(Box::new(WindowTarget(w.clone())))
        }
        None => wgpu::InstanceDescriptor::new_without_display_handle(),
    };
    desc.backends = backends;
    // `WGPU_BACKEND=noop`: a device that runs nothing (wgpu's noop backend), for timing the
    // renderer's CPU side where the only GPU is a software one that would swamp it.
    if backends == wgpu::Backends::NOOP {
        desc.backend_options.noop = noop_options();
    }
    desc.flags = wgpu::InstanceFlags::from_build_config().with_env();
    let instance = wgpu::Instance::new(desc);

    let surface = match &window {
        Some(w) => Some(
            instance
                .create_surface(WindowTarget(w.clone()))
                .map_err(|e| format!("could not create a window surface: {e}"))?,
        ),
        None => None,
    };

    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: surface.as_ref(),
        apply_limit_buckets: false,
    }))
    .map_err(|e| {
        format!(
            "no compatible GPU adapter ({e}). On a machine without a GPU install a software \
             Vulkan driver, e.g. `apt install mesa-vulkan-drivers`."
        )
    })?;
    let info = adapter.get_info();
    tracing::info!(
        "GPU: {} ({:?}, {:?}, driver {})",
        info.name,
        info.backend,
        info.device_type,
        info.driver
    );

    let wanted = wgpu::Features::DEPTH_CLIP_CONTROL
        | wgpu::Features::FLOAT32_FILTERABLE
        | wgpu::Features::RG11B10UFLOAT_RENDERABLE
        | wgpu::Features::TIMESTAMP_QUERY
        | wgpu::Features::TEXTURE_COMPRESSION_BC
        | wgpu::Features::INDIRECT_FIRST_INSTANCE;
    let required_features = adapter.features() & wanted;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("engine device"),
        required_features,
        required_limits: adapter.limits(),
        memory_hints: memory_hints(),
        ..Default::default()
    }))
    .map_err(|e| format!("could not open the GPU device: {e}"))?;

    device.on_uncaptured_error(Arc::new(|e| {
        tracing::error!("GPU error: {e}");
    }));

    let surface = match surface {
        Some(surface) => {
            let mut config = surface
                .get_default_config(&adapter, width.max(1), height.max(1))
                .ok_or("the window surface is not supported by this GPU")?;
            let caps = surface.get_capabilities(&adapter);
            // Prefer an sRGB swapchain; otherwise render through an sRGB view.
            if let Some(f) = caps.formats.iter().find(|f| f.is_srgb()) {
                config.format = *f;
            }
            let view_format = config.format.add_srgb_suffix();
            if view_format != config.format {
                config.view_formats.push(view_format);
            }
            config.present_mode = if vsync {
                wgpu::PresentMode::AutoVsync
            } else {
                wgpu::PresentMode::AutoNoVsync
            };
            config.desired_maximum_frame_latency = 2;
            surface.configure(&device, &config);
            Some(SurfaceState {
                surface,
                config,
                view_format,
            })
        }
        None => None,
    };

    Ok((
        Gpu {
            instance,
            adapter,
            device,
            queue,
            info,
        },
        surface,
    ))
}
