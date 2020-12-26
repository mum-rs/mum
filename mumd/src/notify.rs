pub fn init() {
    #[cfg(feature = "notifications")]
    libnotify::init("mumd").unwrap(); //TODO handle panic (don't send notifications)
}

#[cfg(feature = "notifications")]
pub fn send(msg: String) -> Option<bool> {
    match libnotify::Notification::new("mumd", Some(msg.as_str()), None).show() {
        Ok(_) => Some(true),
        Err(_) => {
            log::debug!("Unable to send notification");
            Some(false)
        }
    }
}

#[cfg(not(feature = "notifications"))]
pub fn send(_: String) -> Option<bool> {
    None
}
