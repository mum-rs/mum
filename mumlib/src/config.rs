//! Representations of the mumdrc configuration file.

use crate::error::ConfigError;
use crate::DEFAULT_PORT;

use log::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

/// The mumdrc config file.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    /// Whether we allow connecting to servers with invalid server certificates.
    ///
    /// None implies false but we can show a better message to the user.
    pub allow_invalid_server_cert: Option<bool>,
    /// General audio configuration.
    #[serde(default)]
    pub audio: Option<AudioConfig>,
    /// Saved servers.
    pub servers: Option<Vec<ServerConfig>>,
}

/// Overwrite a specific sound effect with a file that should be played instead.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SoundEffect {
    /// During which event the effect should be played.
    pub event: String,
    /// The file that should be played.
    pub file: String,
}

/// General audio configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AudioConfig {
    /// The microphone input sensitivity.
    pub input_volume: Option<f32>,
    /// The output main gain.
    pub output_volume: Option<f32>,
    /// Overridden sound effects.
    pub sound_effects: Option<Vec<SoundEffect>>,
    /// If we should disable the noise gate, i.e. send _all_ data from the input to the server.
    pub disable_noise_gate: Option<bool>,
}

/// A saved server.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    /// The alias of the server.
    pub name: String,
    /// The host (URL or IP-address) of the server.
    pub host: String,
    /// The port, if non-default.
    pub port: Option<u16>,
    /// The username to connect with. Prompted on connection if omitted.
    pub username: Option<String>,
    /// The password to connect with. Nothing is sent to the server if omitted.
    pub password: Option<String>,
    /// Whether to accept invalid server certifications for this server.
    pub accept_invalid_cert: Option<bool>,
}

impl ServerConfig {
    /// Creates a [SocketAddr] for this server.
    ///
    /// Returns `None` if no resolution could be made. See
    /// [std::net::ToSocketAddrs] for more information.
    pub fn to_socket_addr(&self) -> Option<SocketAddr> {
        (self.host.as_str(), self.port.unwrap_or(DEFAULT_PORT))
            .to_socket_addrs()
            .ok()?
            .next()
    }
}

/// Finds the default path of the configuration file.
///
/// The user config dir is looked for first (cross-platform friendly) and
/// `/etc/mumdrc` is used as a fallback.
pub fn default_cfg_path() -> PathBuf {
    match dirs::config_dir() {
        Some(mut p) => {
            p.push("mumdrc");
            p
        }
        //TODO This isn't cross platform.
        None => PathBuf::from("/etc/mumdrc"),
    }
}

/// Reads the config at the specified path.
///
/// If the file isn't found, returns a default config.
///
/// # Errors
///
/// - Any [ConfigError::TomlErrorDe] encountered when deserializing the config.
/// - Any [ConfigError::IOError] encountered when reading the file.
pub fn read_cfg(path: &Path) -> Result<Config, ConfigError> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(toml_edit::de::from_str(&s)?),
        Err(e) => {
            if matches!(e.kind(), std::io::ErrorKind::NotFound) && !path.exists() {
                warn!("Config file not found");
            } else {
                error!("Error reading config file: {}", e);
            }
            Ok(Config::default())
        }
    }
}
