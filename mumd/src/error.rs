use std::fmt;

pub enum AudioStream {
    Input,
    Output,
}

impl fmt::Display for AudioStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioStream::Input => write!(f, "input"),
            AudioStream::Output => write!(f, "output"),
        }
    }
}

pub enum AudioError {
    NoDevice(AudioStream),
    NoConfigs(AudioStream, cpal::SupportedStreamConfigsError),
    NoSupportedConfig(AudioStream),
    InvalidStream(AudioStream, cpal::BuildStreamError),
    OutputPlayError(cpal::PlayStreamError),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::NoDevice(s) => write!(f, "No {} device", s),
            AudioError::NoConfigs(s, e) => write!(f, "No {} configs: {}", s, e),
            AudioError::NoSupportedConfig(s) => write!(f, "No supported {} config found", s),
            AudioError::InvalidStream(s, e) => write!(f, "Invalid {} stream: {}", s, e),
            AudioError::OutputPlayError(e) => write!(f, "Playback error: {}", e),
        }
    }
}

pub enum StateError {
    AudioError(AudioError),
}

impl From<AudioError> for StateError {
    fn from(e: AudioError) -> Self {
        StateError::AudioError(e)
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::AudioError(e) => write!(f, "Audio error: {}", e),
        }
    }
}

