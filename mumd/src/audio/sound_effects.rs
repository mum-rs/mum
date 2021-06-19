use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use log::warn;
use mumlib::config::SoundEffect;
use std::borrow::Cow;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

use crate::audio::SAMPLE_RATE;

enum AudioFileKind {
    Wav,
}

impl TryFrom<&str> for AudioFileKind {
    type Error = ();

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            ".wav" => Ok(AudioFileKind::Wav),
            _ => Err(()),
        }
    }
}

struct AudioSpec {
    channels: u32,
    sample_rate: u32,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy, Hash, EnumIter)]
pub enum NotificationEvents {
    ServerConnect,
    ServerDisconnect,
    UserConnected,
    UserDisconnected,
    UserJoinedChannel,
    UserLeftChannel,
    Mute,
    Unmute,
    Deafen,
    Undeafen,
}

impl TryFrom<&str> for NotificationEvents {
    type Error = ();

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "server_connect" => Ok(NotificationEvents::ServerConnect),
            "server_disconnect" => Ok(NotificationEvents::ServerDisconnect),
            "user_connected" => Ok(NotificationEvents::UserConnected),
            "user_disconnected" => Ok(NotificationEvents::UserDisconnected),
            "user_joined_channel" => Ok(NotificationEvents::UserJoinedChannel),
            "user_left_channel" => Ok(NotificationEvents::UserLeftChannel),
            "mute" => Ok(NotificationEvents::Mute),
            "unmute" => Ok(NotificationEvents::Unmute),
            "deafen" => Ok(NotificationEvents::Deafen),
            "undeafen" => Ok(NotificationEvents::Undeafen),
            _ => {
                warn!("Unknown notification event '{}' in config", s);
                Err(())
            }
        }
    }
}

pub fn load_sound_effects(overrides: &[SoundEffect], num_channels: usize) -> HashMap<NotificationEvents, Vec<f32>> {
    let overrides: HashMap<_, _> = overrides
        .iter()
        .filter_map(|sound_effect| {
            let (event, file) = (&sound_effect.event, &sound_effect.file);
            if let Ok(event) = NotificationEvents::try_from(event.as_str()) {
                Some((event, file))
            } else {
                None
            }
        })
        .collect();

    // Construct a hashmap that maps every [NotificationEvent] to a vector of
    // plain floating point audio data with the global sample rate as a
    // Vec<f32>. We do this by iterating over all [NotificationEvent]-variants
    // and opening either the file passed as an override or the fallback sound
    // effect (if omitted). We then use dasp to convert to the correct sample rate.
    NotificationEvents::iter()
        .map(|event| {
            // Open the file if overriden, otherwise use the default sound effect.
            let file = overrides.get(&event);
            let (data, kind) = file
                .map(|file| (
                    get_sfx(file),
                    AudioFileKind::Wav
                ))
                .unwrap_or_else(|| (get_default_sfx(), AudioFileKind::Wav));
            // Unpack the samples.
            let (samples, spec) = unpack_audio(data, kind);
            // If the audio is mono (single channel), pad every sample with
            // itself, since we later assume that audio is stored as LRLRLR (or
            // RLRLRL). Without this, mono audio would be played in double
            // speed.
            let iter: Box<dyn Iterator<Item = f32>> = match spec.channels {
                1 => Box::new(samples.into_iter().flat_map(|e| vec![e, e])),
                2 => Box::new(samples.into_iter()),
                _ => unimplemented!("Only mono and stereo sound is supported. See #80."),
            };
            // Create a dasp signal containing stereo sound.
            let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
            // Create a linear interpolator, in case we need to convert the sample rate.
            let interp = Linear::new(Signal::next(&mut signal), Signal::next(&mut signal));
            // Create our resulting samples.
            let samples = signal
                .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                .until_exhausted()
                // If the source audio is stereo and is being played as mono, discard the first channel.
                .flat_map(|e| {
                    if num_channels == 1 {
                        vec![e[0]]
                    } else {
                        e.to_vec()
                    }
                })
                .collect::<Vec<f32>>();
            (event, samples)
        })
        .collect()
}

/// Unpack audio data. The required audio spec is read from the file and returned as well.
fn unpack_audio(data: Cow<[u8]>, kind: AudioFileKind) -> (Vec<f32>, AudioSpec) {
    match kind {
        AudioFileKind::Wav => unpack_wav(data),
    }
}

/// Unpack wav data.
fn unpack_wav(data: Cow<[u8]>) -> (Vec<f32>, AudioSpec) {
    let reader = hound::WavReader::new(data.as_ref()).unwrap();
    let spec = reader.spec();
    let samples = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|e| e.unwrap())
            .collect::<Vec<_>>(),
        hound::SampleFormat::Int => reader
            .into_samples::<i16>()
            .map(|e| cpal::Sample::to_f32(&e.unwrap()))
            .collect::<Vec<_>>(),
    };
    let spec = AudioSpec {
        channels: spec.channels as u32,
        sample_rate: spec.sample_rate,
    };
    (samples, spec)
}

/// Open and return the data contained in a file, or the default sound effect if
/// the file couldn't be found.
// moo
fn get_sfx<P: AsRef<Path>>(file: P) -> Cow<'static, [u8]> {
    let mut buf: Vec<u8> = Vec::new();
    if let Ok(mut file) = File::open(file.as_ref()) {
        file.read_to_end(&mut buf).unwrap();
        Cow::from(buf)
    } else {
        warn!("File not found: '{}'", file.as_ref().display());
        get_default_sfx()
    }
}

/// Get the default sound effect.
fn get_default_sfx() -> Cow<'static, [u8]> {
    Cow::from(include_bytes!("fallback_sfx.wav").as_ref())
}
