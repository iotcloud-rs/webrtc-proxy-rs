use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Settings {
    pub peer_name: String,
    pub signaling_server_url: String,
    pub stun_server_url: String,
}
