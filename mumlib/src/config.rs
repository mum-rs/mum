use serde::{Deserialize, Serialize};
use std::fs;
use toml::Value;
use toml::value::Array;

#[derive(Debug, Deserialize, Serialize)]
struct TOMLConfig {
    audio: Option<AudioConfig>,
    servers: Option<Array>,
}

#[derive(Debug)]
pub struct Config {
    pub audio: Option<AudioConfig>,
    pub servers: Option<Vec<ServerConfig>>,
}

impl Config {
    pub fn write_default_cfg(&self) {
        fs::write(get_cfg_path(), toml::to_string(&TOMLConfig::from(self)).unwrap()).unwrap();
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
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

fn get_cfg_path() -> String {
    ".mumdrc".to_string() //TODO XDG_CONFIG and whatever
}

impl From<TOMLConfig> for Config {
    fn from(config: TOMLConfig) -> Self {
        Config {
            audio: config.audio,
            servers: if let Some(servers) = config.servers {
                Some(servers
                    .into_iter()
                    .map(|s| s.try_into::<ServerConfig>().expect("invalid server config format"))
                    .collect())
            } else {
                None
            },
        }
    }
}

impl From<Config> for TOMLConfig {
    fn from(config: Config) -> Self {
        TOMLConfig {
            audio: config.audio,
            servers: if let Some(servers) = config.servers {
                Some(servers
                    .into_iter()
                    .map(|s| Value::try_from::<ServerConfig>(s).unwrap())
                    .collect())
            } else {
                None
            },
        }
    }
}

impl From<&Config> for TOMLConfig {
    fn from(config: &Config) -> Self {
        TOMLConfig {
            audio: config.audio.clone(),
            servers: if let Some(servers) = config.servers.clone() {
                Some(servers
                    .into_iter()
                    .map(|s| Value::try_from::<ServerConfig>(s).unwrap())
                    .collect())
            } else {
                None
            },
        }
    }
}

pub fn read_default_cfg() -> Config {
    //TODO ignore when config file doesn't exist
    Config::from(
        toml::from_str::<TOMLConfig>(
            &fs::read_to_string(
                get_cfg_path())
                .expect("config file not found")
                .to_string())
            .unwrap())
}
