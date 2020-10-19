use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fs;
use toml::Value;
use toml::value::Array;

#[derive(Debug, Deserialize, Serialize)]
struct TOMLConfig {
    audio: Option<AudioConfig>,
    servers: Option<Array>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub audio: Option<AudioConfig>,
    pub servers: Option<Vec<ServerConfig>>,
}

impl Config {
    pub fn write_default_cfg(&self) {
        fs::write(get_cfg_path(), toml::to_string(&TOMLConfig::from(self.clone())).unwrap()).unwrap();
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

fn get_cfg_path() -> String {
    ".mumdrc".to_string() //TODO XDG_CONFIG and whatever
}

impl TryFrom<TOMLConfig> for Config {
    type Error = toml::de::Error;

    fn try_from(config: TOMLConfig) -> Result<Self, Self::Error> {
        Ok(Config {
            audio: config.audio,
            servers: config.servers.map(|servers| servers
                                        .into_iter()
                                        .map(|s| s.try_into::<ServerConfig>())
                                        .collect())
                                   .transpose()?,
        })
    }
}

impl From<Config> for TOMLConfig {
    fn from(config: Config) -> Self {
        TOMLConfig {
            audio: config.audio,
            servers: config.servers.map(|servers| servers
                                        .into_iter()
                                        .map(|s| Value::try_from::<ServerConfig>(s).unwrap())
                                        .collect()),
        }
    }
}

pub fn read_default_cfg() -> Result<Config, toml::de::Error> {
    //TODO return None if file doesn't exist (Option::map)
    Config::try_from(
        toml::from_str::<TOMLConfig>(
            &fs::read_to_string(
                get_cfg_path())
                .expect("config file not found")
                .to_string())
            .unwrap())
}
