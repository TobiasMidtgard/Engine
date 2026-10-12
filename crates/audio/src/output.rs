//! Where the mix goes: the sound card (device output, on its own thread) or an offline
//! renderer that advances one frame at a time with the game clock (headless runs, tests and
//! recordings, so a recording made at any frame rate plays back in real time and in sync).

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender};

use crate::mixer::{Command, Mixer, MixerEvent};

/// The running output.
pub(crate) enum Backend {
    #[cfg_attr(
        not(any(windows, target_os = "macos", feature = "alsa")),
        allow(dead_code)
    )]
    Device(DeviceOutput),
    Offline(Box<Mutex<Mixer>>),
}

impl Backend {
    pub fn name(&self) -> String {
        match self {
            Backend::Device(d) => format!("device: {}", d.name),
            Backend::Offline(_) => "offline".into(),
        }
    }
}

/// A sound card stream running on a dedicated thread. Dropping it stops the stream.
pub(crate) struct DeviceOutput {
    pub name: String,
    shutdown: Option<Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for DeviceOutput {
    fn drop(&mut self) {
        self.shutdown.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Open the default output device. Returns the output and its sample rate, or why not.
#[cfg(any(windows, target_os = "macos", feature = "alsa"))]
pub(crate) fn open_device(
    commands: Receiver<Command>,
    events: Sender<MixerEvent>,
) -> Result<(DeviceOutput, f32), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

    fn build<T: SizedSample + FromSample<f32>>(
        device: &cpal::Device,
        config: StreamConfig,
        mut mixer: Mixer,
    ) -> Result<cpal::Stream, cpal::Error> {
        let channels = config.channels as usize;
        let mut buf: Vec<f32> = Vec::new();
        device.build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                if buf.len() < data.len() {
                    buf.resize(data.len(), 0.0);
                }
                let b = &mut buf[..data.len()];
                mixer.process(b, channels);
                for (o, s) in data.iter_mut().zip(b.iter()) {
                    *o = T::from_sample(*s);
                }
            },
            |e| engine_core::log::warn!("audio output: {e}"),
            None,
        )
    }

    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(f32, String), String>>(1);
    let (shutdown_tx, shutdown_rx) = crossbeam_channel::bounded::<()>(0);
    let thread = std::thread::Builder::new()
        .name("audio-output".into())
        .spawn(move || {
            let host = cpal::default_host();
            let Some(device) = host.default_output_device() else {
                let _ = ready_tx.send(Err("no default output device".into()));
                return;
            };
            let name = device
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| "default".into());
            let supported = match device.default_output_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("no usable output format: {e}")));
                    return;
                }
            };
            let fs = supported.sample_rate() as f32;
            let config = supported.config();
            let mixer = Mixer::new(fs, commands, events);
            let stream = match supported.sample_format() {
                SampleFormat::F32 => build::<f32>(&device, config, mixer),
                SampleFormat::F64 => build::<f64>(&device, config, mixer),
                SampleFormat::I16 => build::<i16>(&device, config, mixer),
                SampleFormat::I32 => build::<i32>(&device, config, mixer),
                SampleFormat::U16 => build::<u16>(&device, config, mixer),
                SampleFormat::U8 => build::<u8>(&device, config, mixer),
                SampleFormat::I8 => build::<i8>(&device, config, mixer),
                other => {
                    let _ = ready_tx.send(Err(format!("unsupported sample format {other}")));
                    return;
                }
            };
            let stream = match stream {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("could not open the output stream: {e}")));
                    return;
                }
            };
            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("could not start the output stream: {e}")));
                return;
            }
            let _ = ready_tx.send(Ok((fs, name)));
            // Keep the stream alive until the game drops its handle.
            let _ = shutdown_rx.recv();
            drop(stream);
        })
        .map_err(|e| format!("could not start the audio thread: {e}"))?;
    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok((fs, name))) => Ok((
            DeviceOutput {
                name,
                shutdown: Some(shutdown_tx),
                thread: Some(thread),
            },
            fs,
        )),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("the sound card did not answer within 5 s".into()),
    }
}

/// Device output isn't compiled in (Linux without the `alsa` feature).
#[cfg(not(any(windows, target_os = "macos", feature = "alsa")))]
pub(crate) fn open_device(
    _commands: Receiver<Command>,
    _events: Sender<MixerEvent>,
) -> Result<(DeviceOutput, f32), String> {
    Err(
        "device output is not built in on this platform; build engine_audio with \
         `--features alsa` (needs libasound2-dev) to hear sound on Linux"
            .into(),
    )
}
