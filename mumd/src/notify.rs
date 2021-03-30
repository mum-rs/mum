use log::*;

pub fn init() {
    #[cfg(feature = "notifications")]
    if libnotify::init("mumd").is_err() {
        warn!("Unable to initialize notifications");
    }
}

#[cfg(feature = "notifications")]
pub fn send(msg: String) -> Option<bool> {
    match libnotify::Notification::new("mumd", Some(msg.as_str()), None).show() {
        Ok(_) => Some(true),
        Err(_) => {
            warn!("Unable to send notification");
            Some(false)
        }
    }
}

#[cfg(not(feature = "notifications"))]
pub fn send(_: String) -> Option<bool> {
    None
}
