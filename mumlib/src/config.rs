use crate::error::ConfigError;
use crate::DEFAULT_PORT;

use log::*;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
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

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub audio: AudioConfig,
    pub servers: Vec<ServerConfig>,
    /// Whether we allow connecting to servers with invalid server certificates.
    ///
    /// None implies false but we can show a better message to the user.
    pub allow_invalid_server_cert: Option<bool>,
}

impl Config {
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SoundEffect {
    pub event: String,
    pub file: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AudioConfig {
    pub input_volume: Option<f32>,
    pub output_volume: Option<f32>,
    pub sound_effects: Option<Vec<SoundEffect>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub accept_invalid_cert: Option<bool>,
}

impl ServerConfig {
    pub fn to_socket_addr(&self) -> Option<SocketAddr> {
        match (self.host.as_str(), self.port.unwrap_or(DEFAULT_PORT))
            .to_socket_addrs()
            .map(|mut e| e.next())
        {
            Ok(Some(addr)) => Some(addr),
            _ => None,
        }
    }
}

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
