//! Representations of the mumdrc configuration file.

use crate::error::ConfigError;
use crate::DEFAULT_PORT;

use log::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use toml::value::Array;
use toml::Value;

/// A TOML-friendly version of [Config].
///
/// Values need to be placed before tables due to how TOML works.
#[derive(Debug, Deserialize, Serialize)]
struct TOMLConfig {
    // Values
    accept_all_invalid_certs: Option<bool>,

    // Tables
    audio: Option<AudioConfig>,
    servers: Option<Array>,
}

/// Our representation of the mumdrc config file.
// Deserialized via [TOMLConfig].
#[derive(Clone, Debug, Default)]
pub struct Config {
    /// General audio configuration.
    pub audio: AudioConfig,
    /// Saved servers.
    pub servers: Vec<ServerConfig>,
    /// Whether we allow connecting to servers with invalid server certificates.
    ///
    /// None implies false but we can show a better message to the user.
    pub allow_invalid_server_cert: Option<bool>,
}

impl Config {
    /// Writes this config to the specified path.
    ///
    /// Pass create = true if you want the file to be created if it doesn't already exist.
    ///
    /// # Errors
    ///
    /// - [ConfigError::WontCreateFile] if the file doesn't exist and create = false was passed.
    /// - Any [ConfigError::TOMLErrorSer] encountered when serializing the config.
    /// - Any [ConfigError::IOError] encountered when writing the file.
    pub fn write(&self, path: &Path, create: bool) -> Result<(), ConfigError> {
        // Possible race here. It's fine since it shows when:
        //   1) the file doesn't exist when checked and is then created
        //   2) the file exists when checked but is then removed
        // If 1) we don't do anything anyway so it's fine, and if 2) we
        // immediately re-create the file which, while not perfect, at least
        // should work. Unless the file is removed AND the permissions
        // change, but then we don't have permissions so we can't
        // do anything anyways.

        if !create && !path.exists() {
            return Err(ConfigError::WontCreateFile);
        }

        Ok(fs::write(
            path,
            toml::to_string(&TOMLConfig::from(self.clone()))?,
        )?)
    }
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
    /// Overriden sound effects.
    pub sound_effects: Option<Vec<SoundEffect>>,
}

/// A saved server.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    /// The alias of the server.
    pub name: String,
    /// The host (URL or IP-adress) of the server.
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

impl TryFrom<TOMLConfig> for Config {
    type Error = toml::de::Error;

    fn try_from(config: TOMLConfig) -> Result<Self, Self::Error> {
        Ok(Config {
            audio: config.audio.unwrap_or_default(),
            servers: config
                .servers
                .map(|servers| {
                    servers
                        .into_iter()
                        .map(|s| s.try_into::<ServerConfig>())
                        .collect()
                })
                .transpose()?
                .unwrap_or_default(),
            allow_invalid_server_cert: config.accept_all_invalid_certs,
        })
    }
}

impl From<Config> for TOMLConfig {
    fn from(config: Config) -> Self {
        TOMLConfig {
            audio: if config.audio.output_volume.is_some() || config.audio.input_volume.is_some() {
                Some(config.audio)
            } else {
                None
            },
            servers: Some(
                config
                    .servers
                    .into_iter()
                    // Safe since all ServerConfigs are valid TOML
                    .map(|s| Value::try_from::<ServerConfig>(s).unwrap())
                    .collect(),
            ),
            accept_all_invalid_certs: config.allow_invalid_server_cert,
        }
    }
}

/// Reads the config at the specified path.
///
/// If the file isn't found, returns a default config.
///
/// # Errors
///
/// - Any [ConfigError::TOMLErrorDe] encountered when deserializing the config.
/// - Any [ConfigError::IOError] encountered when reading the file.
pub fn read_cfg(path: &Path) -> Result<Config, ConfigError> {
    match fs::read_to_string(path) {
        Ok(s) => {
            let toml_config: TOMLConfig = toml::from_str(&s)?;
            Ok(Config::try_from(toml_config)?)
        }
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
