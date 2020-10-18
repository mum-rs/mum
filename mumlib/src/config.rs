use serde::Deserialize;
use std::fs;
use toml::value::Array;

#[derive(Debug, Deserialize)]
struct TOMLConfig {
    audio: Option<AudioConfig>,
    servers: Option<Array>,
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub audio: Option<AudioConfig>,
    pub servers: Option<Vec<ServerConfig>>,
}

#[derive(Debug, Deserialize)]
pub struct AudioConfig {
    pub input_volume: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

fn get_cfg_path() -> String {
    "~/.mumdrc".to_string() //TODO XDG_CONFIG and whatever
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
