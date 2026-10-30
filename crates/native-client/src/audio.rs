//! Non-blocking UI sound playback using the selected Minecraft resources.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
};

use anyhow::{Context, Result, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use native_core::random::LegacyRandom;
use native_resources::{
    asset_index::AssetIndex,
    packs::{ResourceId, ResourceStack},
    sounds::{SoundDefinitions, read_sound_file},
};

const MAX_SAMPLES: usize = 4_000_000;
const MAX_VOICES: usize = 64;
#[cfg(test)]
static TEST_RENDERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn reset_test_rendered() {
    TEST_RENDERED.store(false, Ordering::Relaxed);
}

#[cfg(test)]
pub(crate) fn test_rendered() -> bool {
    TEST_RENDERED.load(Ordering::Relaxed)
}

#[derive(Clone, PartialEq, Eq)]
struct Resources {
    root: PathBuf,
    packs: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Listener {
    eye: glam::DVec3,
    yaw: f32,
}

/// One game-thread writer; the callback reads at most once and keeps its last
/// complete value if a write overlaps. No audio-thread locks or retry loops.
#[derive(Default)]
struct SharedListener {
    sequence: AtomicU64,
    x: AtomicU64,
    y: AtomicU64,
    z: AtomicU64,
    yaw: AtomicU32,
}

impl SharedListener {
    fn publish(&self, listener: Listener) {
        if !listener.eye.is_finite() || !listener.yaw.is_finite() {
            return;
        }
        self.sequence.fetch_add(1, Ordering::SeqCst);
        self.x.store(listener.eye.x.to_bits(), Ordering::SeqCst);
        self.y.store(listener.eye.y.to_bits(), Ordering::SeqCst);
        self.z.store(listener.eye.z.to_bits(), Ordering::SeqCst);
        self.yaw.store(listener.yaw.to_bits(), Ordering::SeqCst);
        self.sequence.fetch_add(1, Ordering::SeqCst);
    }

    fn read(&self, previous: Listener) -> Listener {
        let start = self.sequence.load(Ordering::SeqCst);
        if start & 1 != 0 {
            return previous;
        }
        let listener = Listener {
            eye: glam::DVec3::new(
                f64::from_bits(self.x.load(Ordering::SeqCst)),
                f64::from_bits(self.y.load(Ordering::SeqCst)),
                f64::from_bits(self.z.load(Ordering::SeqCst)),
            ),
            yaw: f32::from_bits(self.yaw.load(Ordering::SeqCst)),
        };
        if start == self.sequence.load(Ordering::SeqCst) {
            listener
        } else {
            previous
        }
    }
}

#[derive(Clone)]
pub struct Audio {
    commands: mpsc::SyncSender<Command>,
    gain: Arc<AtomicU32>,
    block_gain: Arc<AtomicU32>,
    player_gain: Arc<AtomicU32>,
    listener: Arc<SharedListener>,
}

#[derive(Clone)]
pub struct UiAudio {
    audio: Audio,
    root: PathBuf,
    packs: Vec<PathBuf>,
}

impl UiAudio {
    pub fn new(audio: Audio, root: PathBuf, packs: Vec<PathBuf>) -> Self {
        Self { audio, root, packs }
    }
    pub fn click(&self) {
        self.audio.click(&self.root, &self.packs);
    }
    pub fn set_gain(&self, gain: f32) {
        self.audio.set_gain(gain);
    }
    pub fn set_block_gain(&self, gain: f32) {
        self.audio.set_block_gain(gain);
    }
    pub fn set_player_gain(&self, gain: f32) {
        self.audio.set_player_gain(gain);
    }
    pub fn set_listener(&self, eye: glam::DVec3, yaw: f32) {
        self.audio.listener.publish(Listener { eye, yaw });
    }
    pub fn button(&self, event: native_session::loaded::ButtonSoundEvent) {
        let _ = self.audio.commands.try_send(Command::Button(
            Resources {
                root: self.root.clone(),
                packs: self.packs.clone(),
            },
            event,
        ));
    }
    pub fn item_pickup(&self, position: glam::DVec3) {
        self.pickup(position, PickupSound::Item);
    }
    pub fn experience_pickup(&self, position: glam::DVec3) {
        self.pickup(position, PickupSound::Experience);
    }
    fn pickup(&self, position: glam::DVec3, kind: PickupSound) {
        let _ = self.audio.commands.try_send(Command::Pickup(
            Resources {
                root: self.root.clone(),
                packs: self.packs.clone(),
            },
            position,
            kind,
        ));
    }
}

#[derive(Clone, Copy)]
enum PickupSound {
    Item,
    Experience,
}

enum Command {
    Prepare(Resources),
    Click(Resources),
    Button(Resources, native_session::loaded::ButtonSoundEvent),
    Pickup(Resources, glam::DVec3, PickupSound),
}

impl Audio {
    #[cfg(test)]
    pub fn test_channel() -> (Self, AudioProbe) {
        let (commands, receiver) = mpsc::sync_channel(32);
        let gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let block_gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let player_gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let listener = Arc::new(SharedListener::default());
        (
            Self {
                commands,
                gain: gain.clone(),
                block_gain,
                player_gain,
                listener,
            },
            AudioProbe { receiver, gain },
        )
    }
    pub fn new() -> Result<Self> {
        let (commands, receiver) = mpsc::sync_channel(32);
        let gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let output_gain = gain.clone();
        let block_gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let output_block_gain = block_gain.clone();
        let player_gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let output_player_gain = player_gain.clone();
        let listener = Arc::new(SharedListener::default());
        let output_listener = listener.clone();
        std::thread::Builder::new()
            .name("audio-resources".into())
            .spawn(move || {
                if let Err(error) = run(
                    receiver,
                    output_gain,
                    output_block_gain,
                    output_player_gain,
                    output_listener,
                ) {
                    eprintln!("Audio unavailable: {error:#}");
                }
            })?;
        Ok(Self {
            commands,
            gain,
            block_gain,
            player_gain,
            listener,
        })
    }

    pub fn set_gain(&self, gain: f32) {
        self.gain
            .store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn set_block_gain(&self, gain: f32) {
        self.block_gain
            .store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    pub fn set_player_gain(&self, gain: f32) {
        self.player_gain
            .store(gain.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// The UI thread never waits on resource access or the output device.
    pub fn click(&self, root: &Path, packs: &[PathBuf]) {
        let _ = self.commands.try_send(Command::Click(Resources {
            root: root.to_path_buf(),
            packs: packs.to_vec(),
        }));
    }

    pub fn prepare(&self, root: &Path, packs: &[PathBuf]) {
        let _ = self.commands.try_send(Command::Prepare(Resources {
            root: root.to_path_buf(),
            packs: packs.to_vec(),
        }));
    }
}

#[cfg(test)]
pub struct AudioProbe {
    receiver: mpsc::Receiver<Command>,
    gain: Arc<AtomicU32>,
}

#[cfg(test)]
impl AudioProbe {
    pub fn click(&self) -> Option<(PathBuf, Vec<PathBuf>)> {
        match self.receiver.try_recv() {
            Ok(Command::Click(resources)) => Some((resources.root, resources.packs)),
            _ => None,
        }
    }
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
}

struct Clip {
    samples: Vec<i16>,
    channels: usize,
    rate: u32,
}

struct Voice {
    clip: Arc<Clip>,
    frame: f64,
    step: f64,
    volume: f32,
    spatial: Option<SpatialSource>,
    player_sound: bool,
}

#[derive(Clone, Copy, Debug)]
struct SpatialSource {
    position: glam::DVec3,
    radius: f64,
}

impl SpatialSource {
    fn factors(self, listener: Listener) -> (f32, f32) {
        let offset = self.position - listener.eye;
        let distance = offset.length();
        let attenuation = if self.radius > 0.0 {
            (1.0 - distance / self.radius).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let right = glam::DVec3::new(
            f64::from(listener.yaw.cos()),
            0.0,
            f64::from(listener.yaw.sin()),
        );
        let pan = if distance > 1e-8 {
            (offset.dot(right) / distance).clamp(-1.0, 1.0) as f32
        } else {
            0.0
        };
        (attenuation, pan)
    }
}

fn decode(bytes: Vec<u8>) -> Result<Clip> {
    let mut reader = lewton::inside_ogg::OggStreamReader::new(Cursor::new(bytes))?;
    let channels = usize::from(reader.ident_hdr.audio_channels);
    let rate = reader.ident_hdr.audio_sample_rate;
    ensure!(
        (1..=2).contains(&channels) && rate > 0,
        "unsupported OGG format"
    );
    let mut samples = Vec::new();
    while let Some(packet) = reader.read_dec_packet_itl()? {
        ensure!(
            samples.len().saturating_add(packet.len()) <= MAX_SAMPLES,
            "sound is too long"
        );
        samples.extend(packet);
    }
    ensure!(
        !samples.is_empty() && samples.len() % channels == 0,
        "empty or incomplete sound"
    );
    Ok(Clip {
        samples,
        channels,
        rate,
    })
}

fn mix(
    voices: &mut Vec<Voice>,
    output: &mut [f32],
    channels: usize,
    gain: f32,
    block_gain: f32,
    player_gain: f32,
    listener: Listener,
) {
    output.fill(0.0);
    for voice in voices.iter_mut() {
        let frames = voice.clip.samples.len() / voice.clip.channels;
        let (attenuation, pan) = voice
            .spatial
            .map_or((1.0, 0.0), |source| source.factors(listener));
        let volume = voice.volume
            * attenuation
            * if voice.player_sound {
                player_gain
            } else if voice.spatial.is_some() {
                block_gain
            } else {
                gain
            };
        let spatialize = voice.spatial.is_some() && channels >= 2 && voice.clip.channels == 1;
        let left = if spatialize { (1.0 - pan).sqrt() } else { 1.0 };
        let right = if spatialize { (1.0 + pan).sqrt() } else { 1.0 };
        for frame in output.chunks_exact_mut(channels) {
            let index = voice.frame as usize;
            if index >= frames {
                break;
            }
            let next = (index + 1).min(frames - 1);
            let fraction = (voice.frame - index as f64) as f32;
            for (channel, sample) in frame.iter_mut().enumerate() {
                let source_channel = if voice.clip.channels == 1 {
                    0
                } else if channel < 2 {
                    channel
                } else {
                    continue;
                };
                let a = f32::from(voice.clip.samples[index * voice.clip.channels + source_channel])
                    / 32768.0;
                let b = f32::from(voice.clip.samples[next * voice.clip.channels + source_channel])
                    / 32768.0;
                let pan = match channel {
                    0 => left,
                    1 => right,
                    _ => 1.0,
                };
                *sample += (a + (b - a) * fraction) * volume * pan;
            }
            voice.frame += voice.step;
        }
    }
    voices.retain(|voice| voice.frame < (voice.clip.samples.len() / voice.clip.channels) as f64);
    for sample in output {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

fn output_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    receiver: mpsc::Receiver<Voice>,
    gain: Arc<AtomicU32>,
    block_gain: Arc<AtomicU32>,
    player_gain: Arc<AtomicU32>,
    listener: Arc<SharedListener>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels);
    ensure!(channels > 0, "audio device has no channels");
    let mut voices = Vec::new();
    let mut buffer = Vec::new();
    let mut last_listener = Listener::default();
    let stream = device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            while let Ok(voice) = receiver.try_recv() {
                if voices.len() < MAX_VOICES {
                    voices.push(voice);
                }
            }
            buffer.resize(output.len(), 0.0);
            last_listener = listener.read(last_listener);
            mix(
                &mut voices,
                &mut buffer,
                channels,
                f32::from_bits(gain.load(Ordering::Relaxed)),
                f32::from_bits(block_gain.load(Ordering::Relaxed)),
                f32::from_bits(player_gain.load(Ordering::Relaxed)),
                last_listener,
            );
            #[cfg(test)]
            if buffer.iter().any(|sample| *sample != 0.0) {
                TEST_RENDERED.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            for (dst, sample) in output.iter_mut().zip(&buffer) {
                *dst = T::from_sample(*sample);
            }
        },
        |error| eprintln!("Audio output error: {error}"),
        None,
    )?;
    stream.play()?;
    Ok(stream)
}

struct Catalog {
    config: Resources,
    index: AssetIndex,
    stack: ResourceStack,
    sounds: SoundDefinitions,
    decoded: BTreeMap<ResourceId, Arc<Clip>>,
}

impl Catalog {
    fn load(config: Resources) -> Result<Self> {
        let index = AssetIndex::load(&config.root)?;
        let mut stack = ResourceStack::open(&config.root)?;
        for pack in &config.packs {
            stack.push_pack(pack)?;
        }
        let sounds = SoundDefinitions::load_minecraft(&index, &config.root, &mut stack)?;
        Ok(Self {
            config,
            index,
            stack,
            sounds,
            decoded: BTreeMap::new(),
        })
    }

    fn click(&mut self, random: &mut LegacyRandom, output_rate: u32) -> Result<Option<Voice>> {
        self.resolve("ui.button.click", 0.25, 1.0, None, random, output_rate)
    }

    #[cfg(test)]
    fn item_pickup(
        &mut self,
        position: glam::DVec3,
        random: &mut LegacyRandom,
        output_rate: u32,
    ) -> Result<Option<Voice>> {
        self.pickup(position, PickupSound::Item, random, output_rate)
    }

    fn pickup(
        &mut self,
        position: glam::DVec3,
        kind: PickupSound,
        random: &mut LegacyRandom,
        output_rate: u32,
    ) -> Result<Option<Voice>> {
        let difference = random.next_float() - random.next_float();
        let (event, volume, pitch) = match kind {
            PickupSound::Item => ("entity.item.pickup", 0.2, difference * 1.4 + 2.0),
            PickupSound::Experience => {
                ("entity.experience_orb.pickup", 0.1, difference * 0.35 + 0.9)
            }
        };
        self.resolve(event, volume, pitch, Some(position), random, output_rate)
            .map(|voice| {
                voice.map(|mut voice| {
                    voice.player_sound = true;
                    voice
                })
            })
    }

    fn resolve(
        &mut self,
        event: &str,
        base_volume: f32,
        base_pitch: f32,
        spatial: Option<glam::DVec3>,
        random: &mut LegacyRandom,
        output_rate: u32,
    ) -> Result<Option<Voice>> {
        let files = RefCell::new(BTreeMap::new());
        let stack = RefCell::new(&mut self.stack);
        let selected = self.sounds.select(
            &ResourceId::parse(event)?,
            &|id| {
                if self.decoded.contains_key(id) {
                    return true;
                }
                let bytes =
                    read_sound_file(&self.index, &self.config.root, &mut stack.borrow_mut(), id)
                        .ok();
                let exists = bytes.is_some();
                files.borrow_mut().insert(id.clone(), bytes);
                exists
            },
            &mut |bound| random.next_int_bound(bound).expect("positive bound"),
        )?;
        let Some(selected) = selected else {
            return Ok(None);
        };
        let clip = if let Some(clip) = self.decoded.get(&selected.id) {
            clip.clone()
        } else {
            let bytes = files
                .into_inner()
                .remove(&selected.id)
                .flatten()
                .context("sound disappeared")?;
            let clip = Arc::new(decode(bytes)?);
            self.decoded.insert(selected.id.clone(), clip.clone());
            clip
        };
        let product = |factors: &[serde_json::Value]| -> Result<f32> {
            factors.iter().try_fold(1.0_f32, |acc, value| {
                let value = value.as_f64().context("sound factor must be numeric")? as f32;
                ensure!(value.is_finite() && value > 0.0, "invalid sound provider");
                Ok(acc * value)
            })
        };
        let raw_volume = base_volume * product(&selected.volume_factors)?;
        let volume = raw_volume.clamp(0.0, 1.0);
        let pitch = (base_pitch * product(&selected.pitch_factors)?).clamp(0.5, 2.0);
        let spatial = spatial.map(|position| SpatialSource {
            position,
            radius: f64::from(selected.attenuation_distance) * f64::from(raw_volume.max(1.0)),
        });
        Ok(Some(Voice {
            step: f64::from(clip.rate) / f64::from(output_rate) * f64::from(pitch),
            clip,
            frame: 0.0,
            volume,
            spatial,
            player_sound: false,
        }))
    }
}

fn run(
    receiver: mpsc::Receiver<Command>,
    gain: Arc<AtomicU32>,
    block_gain: Arc<AtomicU32>,
    player_gain: Arc<AtomicU32>,
    listener: Arc<SharedListener>,
) -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .context("no default audio device")?;
    let supported = device.default_output_config()?;
    let config = supported.config();
    let (sender, voices) = mpsc::sync_channel(MAX_VOICES);
    let _stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => output_stream::<f32>(
            &device,
            &config,
            voices,
            gain,
            block_gain,
            player_gain,
            listener,
        )?,
        cpal::SampleFormat::I16 => output_stream::<i16>(
            &device,
            &config,
            voices,
            gain,
            block_gain,
            player_gain,
            listener,
        )?,
        cpal::SampleFormat::U16 => output_stream::<u16>(
            &device,
            &config,
            voices,
            gain,
            block_gain,
            player_gain,
            listener,
        )?,
        other => bail!("unsupported output sample format {other:?}"),
    };
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i64;
    let mut random = LegacyRandom::new(seed);
    let mut catalog: Option<Catalog> = None;
    while let Ok(command) = receiver.recv() {
        let (resources, play, button, pickup) = match command {
            Command::Prepare(resources) => (resources, false, None, None),
            Command::Click(resources) => (resources, true, None, None),
            Command::Button(resources, event) => (resources, true, Some(event), None),
            Command::Pickup(resources, position, kind) => {
                (resources, true, None, Some((position, kind)))
            }
        };
        if catalog
            .as_ref()
            .is_none_or(|current| current.config != resources)
        {
            catalog = match Catalog::load(resources) {
                Ok(catalog) => Some(catalog),
                Err(error) => {
                    eprintln!("Cannot load sound resources: {error:#}");
                    None
                }
            };
        }
        if let Some(catalog) = catalog.as_mut() {
            let mut preload_random = LegacyRandom::new(0);
            let chosen_random = if play {
                &mut random
            } else {
                &mut preload_random
            };
            let resolved = if let Some((position, kind)) = pickup {
                catalog.pickup(position, kind, chosen_random, config.sample_rate.0)
            } else {
                match button {
                    Some(event) => catalog.resolve(
                        if event.powered {
                            "block.stone_button.click_on"
                        } else {
                            "block.stone_button.click_off"
                        },
                        1.0,
                        1.0,
                        Some(event.position.as_dvec3() + glam::DVec3::splat(0.5)),
                        chosen_random,
                        config.sample_rate.0,
                    ),
                    None => catalog.click(chosen_random, config.sample_rate.0),
                }
            };
            match resolved {
                Ok(Some(voice)) => {
                    if play {
                        let _ = sender.try_send(voice);
                    }
                }
                Ok(None) => {}
                Err(error) => eprintln!("Cannot play sound: {error:#}"),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixer_resamples_and_finishes_multiple_voices() {
        let clip = Arc::new(Clip {
            samples: vec![0, 16384, 0],
            channels: 1,
            rate: 2,
        });
        let mut voices = vec![Voice {
            clip,
            frame: 0.0,
            step: 0.5,
            volume: 0.5,
            spatial: None,
            player_sound: false,
        }];
        let mut output = [0.0; 12];
        mix(
            &mut voices,
            &mut output,
            2,
            1.0,
            1.0,
            1.0,
            Listener::default(),
        );
        assert_eq!(
            output,
            [
                0.0, 0.0, 0.125, 0.125, 0.25, 0.25, 0.125, 0.125, 0.0, 0.0, 0.0, 0.0
            ]
        );
        assert!(voices.is_empty());
    }

    #[test]
    fn volume_scales_and_mutes_active_voices_before_clipping() {
        let clip = Arc::new(Clip {
            samples: vec![32767; 3],
            channels: 1,
            rate: 1,
        });
        let voice = || Voice {
            clip: clip.clone(),
            frame: 0.0,
            step: 1.0,
            volume: 1.0,
            spatial: None,
            player_sound: false,
        };
        let mut voices = vec![voice(), voice()];
        let mut output = [0.0; 1];
        mix(
            &mut voices,
            &mut output,
            1,
            0.25,
            1.0,
            1.0,
            Listener::default(),
        );
        assert_eq!(output[0], 32767.0 / 32768.0 * 0.5);
        mix(
            &mut voices,
            &mut output,
            1,
            0.0,
            1.0,
            1.0,
            Listener::default(),
        );
        assert_eq!(output[0], 0.0);
        mix(
            &mut voices,
            &mut output,
            1,
            1.0,
            1.0,
            1.0,
            Listener::default(),
        );
        assert_eq!(output[0], 1.0);
        assert!(voices.is_empty());
    }

    #[test]
    fn block_and_ui_gains_and_stereo_direction_are_independent() {
        let clip = Arc::new(Clip {
            samples: vec![16384, 16384],
            channels: 1,
            rate: 1,
        });
        let mut voices = vec![
            Voice {
                clip: clip.clone(),
                frame: 0.0,
                step: 1.0,
                volume: 1.0,
                spatial: None,
                player_sound: false,
            },
            Voice {
                clip,
                frame: 0.0,
                step: 1.0,
                volume: 1.0,
                spatial: Some(SpatialSource {
                    position: glam::DVec3::X,
                    radius: f64::INFINITY,
                }),
                player_sound: false,
            },
        ];
        let mut frame = [0.0; 2];
        mix(
            &mut voices,
            &mut frame,
            2,
            0.5,
            0.0,
            1.0,
            Listener::default(),
        );
        assert_eq!(frame, [0.25, 0.25]);
        mix(
            &mut voices,
            &mut frame,
            2,
            0.0,
            0.5,
            1.0,
            Listener::default(),
        );
        assert_eq!(frame[0], 0.0);
        assert!((frame[1] - 0.25 * 2.0_f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn player_sound_gain_is_independent_and_updates_active_voice() {
        let clip = Arc::new(Clip {
            samples: vec![16384; 2],
            channels: 1,
            rate: 1,
        });
        let mut voices = vec![Voice {
            clip,
            frame: 0.0,
            step: 1.0,
            volume: 1.0,
            spatial: Some(SpatialSource {
                position: glam::DVec3::ZERO,
                radius: 16.0,
            }),
            player_sound: true,
        }];
        let mut output = [0.0; 2];
        mix(
            &mut voices,
            &mut output,
            2,
            0.0,
            0.0,
            0.5,
            Listener::default(),
        );
        assert_eq!(output, [0.25, 0.25]);
        mix(
            &mut voices,
            &mut output,
            2,
            1.0,
            1.0,
            0.0,
            Listener::default(),
        );
        assert_eq!(output, [0.0, 0.0]);
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn item_pickup_uses_resource_pack_and_player_sound() {
        let root = native_resources::default_minecraft_dir().unwrap();
        let mut vanilla = Catalog::load(Resources {
            root: root.clone(),
            packs: Vec::new(),
        })
        .unwrap();
        let default_voice = vanilla
            .item_pickup(glam::DVec3::ZERO, &mut LegacyRandom::new(1), 48000)
            .unwrap()
            .unwrap();
        assert!(default_voice.clip.samples.iter().any(|&sample| sample != 0));
        let pack = tempfile::tempdir().unwrap();
        let directory = pack.path().join("assets/minecraft");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("sounds.json"), br#"{"entity.item.pickup":{"replace":true,"sounds":[{"name":"random/click_stereo","volume":0.4,"pitch":1.5}]}}"#).unwrap();
        let mut catalog = Catalog::load(Resources {
            root,
            packs: vec![pack.path().to_path_buf()],
        })
        .unwrap();
        let mut expected = LegacyRandom::new(42);
        let pitch = ((expected.next_float() - expected.next_float()) * 1.4 + 2.0) * 1.5;
        let position = glam::DVec3::new(17.25, 64.7, -3.125);
        let voice = catalog
            .item_pickup(position, &mut LegacyRandom::new(42), 48000)
            .unwrap()
            .unwrap();
        assert!(voice.player_sound);
        assert_eq!(voice.spatial.unwrap().position, position);
        assert_eq!(voice.volume, 0.2_f32 * 0.4);
        assert_eq!(
            voice.step,
            f64::from(voice.clip.rate) / 48000.0 * f64::from(pitch.clamp(0.5, 2.0))
        );
        assert!(voice.clip.samples.iter().any(|&sample| sample != 0));
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn experience_pickup_uses_resource_pack_pitch_and_player_category() {
        let root = native_resources::default_minecraft_dir().unwrap();
        let position = glam::DVec3::new(17.25, 64.7, -3.125);
        let mut vanilla = Catalog::load(Resources {
            root: root.clone(),
            packs: Vec::new(),
        })
        .unwrap();
        let voice = vanilla
            .pickup(
                position,
                PickupSound::Experience,
                &mut LegacyRandom::new(1),
                48000,
            )
            .unwrap()
            .unwrap();
        assert!(voice.clip.samples.iter().any(|&sample| sample != 0));
        let pack = tempfile::tempdir().unwrap();
        let directory = pack.path().join("assets/minecraft");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("sounds.json"), br#"{"entity.experience_orb.pickup":{"replace":true,"sounds":[{"name":"random/click_stereo","volume":0.4,"pitch":1.5}]}}"#).unwrap();
        let mut catalog = Catalog::load(Resources {
            root,
            packs: vec![pack.path().to_path_buf()],
        })
        .unwrap();
        let mut expected = LegacyRandom::new(42);
        let pitch = ((expected.next_float() - expected.next_float()) * 0.35 + 0.9) * 1.5;
        let voice = catalog
            .pickup(
                position,
                PickupSound::Experience,
                &mut LegacyRandom::new(42),
                48000,
            )
            .unwrap()
            .unwrap();
        assert!(voice.player_sound);
        assert_eq!(voice.spatial.unwrap().position, position);
        assert_eq!(voice.volume, 0.1_f32 * 0.4);
        assert_eq!(
            voice.step,
            f64::from(voice.clip.rate) / 48000.0 * f64::from(pitch.clamp(0.5, 2.0))
        );
        assert!(voice.clip.samples.iter().any(|&sample| sample != 0));
    }

    #[test]
    #[ignore = "requires an available Windows audio output and installed 26.3 assets"]
    fn item_pickup_reaches_output_with_only_player_gain_enabled() {
        let root = native_resources::default_minecraft_dir().unwrap();
        let audio = Audio::new().unwrap();
        audio.prepare(&root, &[]);
        let ui = UiAudio::new(audio, root, Vec::new());
        ui.set_gain(0.0);
        ui.set_block_gain(0.0);
        ui.set_player_gain(1.0);
        reset_test_rendered();
        let position = glam::DVec3::new(1234.25, 64.0, -900.5);
        ui.set_listener(position, 0.0);
        ui.item_pickup(position);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !test_rendered() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            test_rendered(),
            "item sound did not reach the output callback"
        );
    }

    #[test]
    #[ignore = "requires an available Windows audio output and installed 26.3 assets"]
    fn experience_pickup_reaches_output_with_only_player_gain_enabled() {
        let root = native_resources::default_minecraft_dir().unwrap();
        let audio = Audio::new().unwrap();
        audio.prepare(&root, &[]);
        let ui = UiAudio::new(audio, root, Vec::new());
        ui.set_gain(0.0);
        ui.set_block_gain(0.0);
        ui.set_player_gain(1.0);
        reset_test_rendered();
        let position = glam::DVec3::new(1234.25, 64.0, -900.5);
        ui.set_listener(position, 0.0);
        ui.experience_pickup(position);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !test_rendered() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            test_rendered(),
            "experience sound did not reach the output callback"
        );
    }

    #[test]
    fn active_sound_tracks_listener_movement_and_rotation() {
        let clip = Arc::new(Clip {
            samples: vec![16384; 4],
            channels: 1,
            rate: 1,
        });
        let position = glam::DVec3::new(16_000_000.5, 64.5, -16_000_000.5);
        let mut voices = vec![Voice {
            clip,
            frame: 0.0,
            step: 1.0,
            volume: 1.0,
            spatial: Some(SpatialSource {
                position,
                radius: 16.0,
            }),
            player_sound: false,
        }];
        let mut output = [0.0; 2];
        let front = Listener {
            eye: position - glam::DVec3::X * 8.0,
            yaw: 0.0,
        };
        mix(&mut voices, &mut output, 2, 0.0, 1.0, 1.0, front);
        assert_eq!(output[0], 0.0);
        assert!((output[1] - 0.25 * 2.0_f32.sqrt()).abs() < 1e-6);
        mix(
            &mut voices,
            &mut output,
            2,
            0.0,
            1.0,
            1.0,
            Listener {
                yaw: std::f32::consts::PI,
                ..front
            },
        );
        assert!(output[0] > 0.35 && output[1] < 0.001);
        mix(
            &mut voices,
            &mut output,
            2,
            0.0,
            1.0,
            1.0,
            Listener {
                eye: position - glam::DVec3::X * 17.0,
                yaw: 0.0,
            },
        );
        assert_eq!(output, [0.0, 0.0]);
        mix(
            &mut voices,
            &mut output,
            2,
            0.0,
            1.0,
            1.0,
            Listener {
                eye: position,
                yaw: 0.0,
            },
        );
        assert_eq!(output, [0.5, 0.5]);
        assert!(voices.is_empty());
    }

    #[test]
    fn listener_reads_only_complete_snapshots_and_never_waits_on_a_writer() {
        let shared = Arc::new(SharedListener::default());
        let before = Listener {
            eye: glam::DVec3::new(3.0, 6.0, -3.0),
            yaw: 3.0,
        };
        shared.publish(before);
        shared.sequence.fetch_add(1, Ordering::SeqCst);
        assert_eq!(shared.read(before), before);
        shared.sequence.fetch_add(1, Ordering::SeqCst);
        shared.publish(Listener {
            eye: glam::DVec3::splat(f64::NAN),
            yaw: 0.0,
        });
        assert_eq!(shared.read(Listener::default()), before);
        let writer = shared.clone();
        let thread = std::thread::spawn(move || {
            for i in 1..=50_000 {
                let x = f64::from(i);
                writer.publish(Listener {
                    eye: glam::DVec3::new(x, x * 2.0, -x),
                    yaw: i as f32,
                });
            }
        });
        let mut last = before;
        for _ in 0..50_000 {
            last = shared.read(last);
            assert_eq!(last.eye.y, last.eye.x * 2.0);
            assert_eq!(last.eye.z, -last.eye.x);
            assert_eq!(f64::from(last.yaw), last.eye.x);
        }
        thread.join().unwrap();
        let last = shared.read(last);
        assert_eq!(last.eye.x, 50_000.0);
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_stone_button_events_select_original_pitch_and_attenuate() {
        let root = native_resources::default_minecraft_dir().unwrap();
        let mut catalog = Catalog::load(Resources {
            root,
            packs: Vec::new(),
        })
        .unwrap();
        for (event, pitch) in [
            ("block.stone_button.click_on", 0.6_f32),
            ("block.stone_button.click_off", 0.5_f32),
        ] {
            let pos = glam::IVec3::ZERO;
            let centered = pos.as_dvec3() + glam::DVec3::splat(0.5);
            let voice = catalog
                .resolve(
                    event,
                    1.0,
                    1.0,
                    Some(centered),
                    &mut LegacyRandom::new(42),
                    48000,
                )
                .unwrap()
                .unwrap();
            assert_eq!(voice.volume, 0.3_f32);
            assert_eq!(
                voice.step,
                f64::from(voice.clip.rate) / 48000.0 * f64::from(pitch)
            );
            let spatial = voice.spatial.unwrap();
            assert!(voice.clip.samples.iter().any(|sample| *sample != 0));
            assert_eq!(
                spatial.factors(Listener {
                    eye: centered,
                    yaw: 0.0
                }),
                (1.0, 0.0)
            );
            assert_eq!(
                spatial.factors(Listener {
                    eye: centered + glam::DVec3::X * 8.0,
                    yaw: 0.0
                }),
                (0.5, -1.0)
            );
            assert_eq!(
                spatial
                    .factors(Listener {
                        eye: centered + glam::DVec3::X * 17.0,
                        yaw: 0.0
                    })
                    .0,
                0.0
            );
        }
    }

    #[test]
    #[ignore = "requires the locally installed Minecraft 26.3 assets"]
    fn installed_click_decodes_with_selected_pack_stack() {
        let root = native_resources::default_minecraft_dir().unwrap();
        let mut catalog = Catalog::load(Resources {
            root,
            packs: Vec::new(),
        })
        .unwrap();
        let mut random = LegacyRandom::new(1);
        let voice = catalog.click(&mut random, 48000).unwrap().unwrap();
        assert!(voice.clip.samples.iter().any(|&sample| sample != 0));
        assert!(voice.volume > 0.0 && voice.step > 0.0);

        let pack = tempfile::tempdir().unwrap();
        let directory = pack.path().join("assets/minecraft");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("sounds.json"), br#"{"ui.button.click":{"replace":true,"sounds":[{"name":"random/click_stereo","volume":0.4,"pitch":1.5}]}}"#).unwrap();
        let mut override_catalog = Catalog::load(Resources {
            root: catalog.config.root.clone(),
            packs: vec![pack.path().to_path_buf()],
        })
        .unwrap();
        let override_voice = override_catalog
            .click(&mut LegacyRandom::new(1), 48000)
            .unwrap()
            .unwrap();
        assert_eq!(override_voice.volume, 0.1);
        assert_eq!(
            override_voice.step,
            f64::from(override_voice.clip.rate) / 48000.0 * 1.5
        );
        let sound_directory = directory.join("sounds/random");
        std::fs::create_dir_all(&sound_directory).unwrap();
        std::fs::write(
            sound_directory.join("click_stereo.ogg"),
            b"invalid OGG override",
        )
        .unwrap();
        let mut invalid = Catalog::load(Resources {
            root: catalog.config.root.clone(),
            packs: vec![pack.path().to_path_buf()],
        })
        .unwrap();
        assert!(invalid.click(&mut LegacyRandom::new(1), 48000).is_err());
    }

    #[test]
    #[ignore = "requires an available Windows audio output and installed 26.3 assets"]
    fn default_device_renders_decoded_click() {
        use std::sync::atomic::Ordering;
        TEST_RENDERED.store(false, Ordering::Relaxed);
        let root = native_resources::default_minecraft_dir().unwrap();
        let audio = Audio::new().unwrap();
        audio.prepare(&root, &[]);
        audio.click(&root, &[]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !TEST_RENDERED.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            TEST_RENDERED.load(Ordering::Relaxed),
            "audio output callback did not render sound"
        );
        drop(audio);
    }

    #[test]
    #[ignore = "requires an available Windows audio output and installed 26.3 assets"]
    fn block_button_sound_reaches_output_device() {
        let root = native_resources::default_minecraft_dir().unwrap();
        let audio = Audio::new().unwrap();
        audio.prepare(&root, &[]);
        let ui = UiAudio::new(audio, root, Vec::new());
        ui.set_gain(0.0);
        ui.set_block_gain(1.0);
        reset_test_rendered();
        let pos = glam::IVec3::new(1234, 64, -900);
        ui.set_listener(pos.as_dvec3() + glam::DVec3::splat(0.5), 0.0);
        ui.button(native_session::loaded::ButtonSoundEvent {
            position: pos,
            powered: true,
            tick: 1,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !test_rendered() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            test_rendered(),
            "block sound did not reach the output callback with UI muted"
        );
    }
}
