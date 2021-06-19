use dasp_interpolate::linear::Linear;
use dasp_signal::{self as signal, Signal};
use log::warn;
use mumlib::config::SoundEffect;
use std::{borrow::Cow, collections::HashMap, convert::TryFrom, fs::File, io::Read};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

use crate::audio::SAMPLE_RATE;

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

    NotificationEvents::iter()
        .map(|event| {
            let bytes = overrides
                .get(&event)
                .map(|file| get_sfx(file))
                .unwrap_or_else(get_default_sfx);
            let reader = hound::WavReader::new(bytes.as_ref()).unwrap();
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
            let iter: Box<dyn Iterator<Item = f32>> = match spec.channels {
                1 => Box::new(samples.into_iter().flat_map(|e| vec![e, e])),
                2 => Box::new(samples.into_iter()),
                _ => unimplemented!("Only mono and stereo sound is supported. See #80."),
            };
            let mut signal = signal::from_interleaved_samples_iter::<_, [f32; 2]>(iter);
            let interp = Linear::new(Signal::next(&mut signal), Signal::next(&mut signal));
            let samples = signal
                .from_hz_to_hz(interp, spec.sample_rate as f64, SAMPLE_RATE as f64)
                .until_exhausted()
                // if the source audio is stereo and is being played as mono, discard the right audio
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

// moo
fn get_sfx(file: &str) -> Cow<'static, [u8]> {
    let mut buf: Vec<u8> = Vec::new();
    if let Ok(mut file) = File::open(file) {
        file.read_to_end(&mut buf).unwrap();
        Cow::from(buf)
    } else {
        warn!("File not found: '{}'", file);
        get_default_sfx()
    }
}

fn get_default_sfx() -> Cow<'static, [u8]> {
    Cow::from(include_bytes!("fallback_sfx.wav").as_ref())
}
