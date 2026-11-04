//! Command encoders finished on worker threads.
//!
//! Since wgpu 27 the work of encoding (validation, resource tracking, the backend's commands)
//! happens in `CommandEncoder::finish`, not as passes are recorded, so one encoder for the whole
//! frame does it all on the render thread at the end. The renderer instead splits the frame into
//! a few encoders (before the shadows, the shadow maps, the depth prepass, the lighting inputs,
//! the opaque scene, the transparent phase, the post chain) and hands each finished part to a
//! worker here while it records the next; the buffers come back in the order they were handed
//! in, for one ordered submit (Unreal translates command lists in parallel the same way and keeps
//! "the order of submission of commands to the GPU ... unchanged"). With the scene in one encoder
//! a mirror view's main thread waited 4.1 ms a frame for it at the lake on Tobias's PC (round 29b),
//! after recording it in under 1 ms. bevy's parallel encoding (one task per pass and
//! view, `finish` on the pool) cut its render-thread time by 26 to 45%
//! (bevyengine/bevy#9172); wgpu supports several encoders on several threads.

use std::sync::{Arc, Mutex, mpsc};

type Job = (usize, &'static str, wgpu::CommandEncoder);

pub(crate) struct Finisher {
    jobs: Option<mpsc::Sender<Job>>,
    /// In a mutex only so the renderer stays `Sync`; one thread uses it.
    done: Mutex<mpsc::Receiver<(usize, wgpu::CommandBuffer)>>,
    /// Buffers finished on the render thread (no workers, or they stopped).
    inline: Vec<(usize, wgpu::CommandBuffer)>,
    /// Encoders handed in since the last `collect`.
    sent: usize,
    workers: Vec<std::thread::JoinHandle<()>>,
}

/// The parts a frame's commands are split into, in order.
pub const PARTS: [&str; 7] = [
    "frame",
    "shadows",
    "prepass",
    "lighting",
    "scene",
    "transparent",
    "post",
];

/// Workers finishing encoders: enough for the parts recorded while earlier ones finish.
pub const THREADS: usize = 4;

/// `encoder.finish()` inside a span naming its part, so traces show what each part costs.
fn finish_traced(part: &str, encoder: wgpu::CommandEncoder) -> wgpu::CommandBuffer {
    let _span = match part {
        "frame" => tracing::info_span!("render: finish frame").entered(),
        "shadows" => tracing::info_span!("render: finish shadows").entered(),
        "prepass" => tracing::info_span!("render: finish prepass").entered(),
        "lighting" => tracing::info_span!("render: finish lighting").entered(),
        "scene" => tracing::info_span!("render: finish scene").entered(),
        "transparent" => tracing::info_span!("render: finish transparent").entered(),
        _ => tracing::info_span!("render: finish post").entered(),
    };
    encoder.finish()
}

impl Finisher {
    /// `threads` workers (0: every encoder is finished where it is handed in).
    pub fn new(threads: usize) -> Self {
        let (jobs, queue) = mpsc::channel::<Job>();
        let queue = Arc::new(Mutex::new(queue));
        let (finished, done) = mpsc::channel();
        let workers: Vec<_> = (0..threads)
            .filter_map(|i| {
                let queue = queue.clone();
                let finished = finished.clone();
                std::thread::Builder::new()
                    .name(format!("render finish {i}"))
                    .spawn(move || {
                        loop {
                            // Wait for a job, then let the next worker wait while this one works.
                            let job = queue.lock().unwrap_or_else(|e| e.into_inner()).recv();
                            let Ok((n, part, encoder)) = job else {
                                break;
                            };
                            let buffer = finish_traced(part, encoder);
                            if finished.send((n, buffer)).is_err() {
                                break;
                            }
                        }
                    })
                    .ok()
            })
            .collect();
        Self {
            jobs: (!workers.is_empty()).then_some(jobs),
            done: Mutex::new(done),
            inline: Vec::new(),
            sent: 0,
            workers,
        }
    }

    /// Finish `encoder` (the frame's part `part`, one of [`PARTS`]) on a worker (or here,
    /// without one).
    pub fn send(&mut self, part: &'static str, encoder: wgpu::CommandEncoder) {
        let n = self.sent;
        self.sent += 1;
        let encoder = match &self.jobs {
            Some(jobs) => match jobs.send((n, part, encoder)) {
                Ok(()) => return,
                Err(mpsc::SendError((_, _, encoder))) => {
                    self.jobs = None;
                    encoder
                }
            },
            None => encoder,
        };
        self.inline.push((n, finish_traced(part, encoder)));
    }

    /// Hand `encoder` (part `*part`) in and go on recording into a new one for part `next`.
    pub fn hand_off(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        part: &mut &'static str,
        next: &'static str,
    ) {
        let new =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(next) });
        self.send(part, std::mem::replace(encoder, new));
        *part = next;
    }

    /// Every buffer handed in since the last call, in the order handed in (waits for the
    /// workers still finishing).
    pub fn collect(&mut self) -> Vec<wgpu::CommandBuffer> {
        let mut out: Vec<Option<wgpu::CommandBuffer>> = (0..self.sent).map(|_| None).collect();
        for (n, b) in self.inline.drain(..) {
            out[n] = Some(b);
        }
        let mut missing = out.iter().filter(|b| b.is_none()).count();
        let done = self.done.get_mut().unwrap_or_else(|e| e.into_inner());
        while missing > 0 {
            let (n, b) = done
                .recv()
                .expect("a render finishing thread stopped with work in hand");
            out[n] = Some(b);
            missing -= 1;
        }
        self.sent = 0;
        out.into_iter().flatten().collect()
    }
}

impl Drop for Finisher {
    fn drop(&mut self) {
        self.jobs = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_come_back_in_the_order_handed_in() {
        let gpu = match crate::gpu::create(None, 1, 1, false) {
            Ok((gpu, _)) => gpu,
            Err(e) => {
                eprintln!("skipping: no GPU ({e})");
                return;
            }
        };
        for threads in [0, 2, THREADS] {
            let mut f = Finisher::new(threads);
            for round in 0..3 {
                for (i, part) in PARTS.into_iter().take(5).enumerate() {
                    f.send(
                        part,
                        gpu.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some(&format!("{round} {i}")),
                            }),
                    );
                }
                let buffers = f.collect();
                assert_eq!(buffers.len(), 5);
                gpu.queue.submit(buffers);
            }
        }
    }
}
