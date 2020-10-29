use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fs;
use std::path::Path;
use toml::value::Array;
use toml::Value;

#[derive(Debug, Deserialize, Serialize)]
struct TOMLConfig {
    audio: Option<AudioConfig>,
    servers: Option<Array>,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub audio: Option<AudioConfig>,
    pub servers: Option<Vec<ServerConfig>>,
}

impl Config {
    pub fn write_default_cfg(&self, create: bool) -> Result<(), std::io::Error> {
        let path = if create {
            get_creatable_cfg_path()
        } else {
            get_cfg_path()
        };
        let path = std::path::Path::new(&path);
        // Possible race here. It's fine since it shows when:
        //   1) the file doesn't exist when checked and is then created
        //   2) the file exists when checked but is then removed
        // If 1) we don't do anything anyway so it's fine, and if 2) we
        // immediately re-create the file which, while not perfect, at least
        // should work. Unless the file is removed AND the permissions
        // change, but then we don't have permissions so we can't
        // do anything anyways.
        if !create && !path.exists() {
            return Ok(());
        }

        fs::write(
            path,
            toml::to_string(&TOMLConfig::from(self.clone())).unwrap(),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AudioConfig {
    pub input_volume: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerConfig {
    pub name: String,
    pub host: String,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub fn get_cfg_path() -> String {
    if let Ok(var) = std::env::var("XDG_CONFIG_HOME") {
        let path = format!("{}/mumdrc", var);
        if Path::new(&path).exists() {
            return path;
        }
    } else if let Ok(var) = std::env::var("HOME") {
        let path = format!("{}/.config/mumdrc", var);
        if Path::new(&path).exists() {
            return path;
        }
    }

    "/etc/mumdrc".to_string()
}

pub fn get_creatable_cfg_path() -> String {
    if let Ok(var) = std::env::var("XDG_CONFIG_HOME") {
        let path = format!("{}/mumdrc", var);
        if !Path::new(&path).exists() {
            return path;
        }
    } else if let Ok(var) = std::env::var("HOME") {
        let path = format!("{}/.config/mumdrc", var);
        if !Path::new(&path).exists() {
            return path;
        }
    }

    "/etc/mumdrc".to_string()
}

pub fn cfg_exists() -> bool {
    if let Ok(var) = std::env::var("XDG_CONFIG_HOME") {
        let path = format!("{}/mumdrc", var);
        if Path::new(&path).exists() {
            return true;
        }
    } else if let Ok(var) = std::env::var("HOME") {
        let path = format!("{}/.config/mumdrc", var);
        if Path::new(&path).exists() {
            return true;
        }
    } else if Path::new("/etc/mumdrc").exists() {
        return true;
    }

    false
}

impl TryFrom<TOMLConfig> for Config {
    type Error = toml::de::Error;

    fn try_from(config: TOMLConfig) -> Result<Self, Self::Error> {
        Ok(Config {
            audio: config.audio,
            servers: config
                .servers
                .map(|servers| {
                    servers
                        .into_iter()
                        .map(|s| s.try_into::<ServerConfig>())
                        .collect()
                })
                .transpose()?,
        })
    }
}

impl From<Config> for TOMLConfig {
    fn from(config: Config) -> Self {
        TOMLConfig {
            audio: config.audio,
            servers: config.servers.map(|servers| {
                servers
                    .into_iter()
                    .map(|s| Value::try_from::<ServerConfig>(s).unwrap())
                    .collect()
            }),
        }
    }
}

pub fn read_default_cfg() -> Option<Config> {
    Some(
        Config::try_from(
            toml::from_str::<TOMLConfig>(&match fs::read_to_string(get_cfg_path()) {
                Ok(f) => f.to_string(),
                Err(_) => return None,
            })
            .expect("invalid TOML in config file"), //TODO
        )
        .expect("invalid config in TOML"),
    ) //TODO
}
