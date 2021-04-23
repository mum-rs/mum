use std::net::SocketAddr;
use tokio::net::lookup_host;

#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub username: String,
    pub password: Option<String>,

    pub host: String,
    pub port: u16,

    pub addr: SocketAddr,

    pub accept_invalid_cert: bool,
}

impl Server {
    pub async fn new(
        host: String,
        port: u16,
        username: String,
        password: Option<String>,
        accept_invalid_cert: bool,
    ) -> Self {
        //TODO config accept host with port? eg: "192.168.1.1:1337"
        let host_format = format!("{}:{}", host, port);
        let addr = lookup_host(host_format).await.unwrap().next().unwrap();

        Self {
            username,
            password,
            host,
            addr,
            port,
            accept_invalid_cert,
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }
}

impl From<&Server> for mumlib::state::Server {
    fn from(_server: &Server) -> Self {
        unimplemented!()
        //mumlib::state::Server {
        //    channels: into_channel(server.channels(), server.users()),
        //    welcome_text: server.welcome_text.clone(),
        //    username: server.username.clone().unwrap(),
        //    host: server.host.as_ref().unwrap().clone(),
        //}
    }
}
