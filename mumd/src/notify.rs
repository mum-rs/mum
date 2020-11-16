pub fn init() {
    #[cfg(any(feature = "notifications", feature = "all"))]
    libnotify::init("mumd").unwrap();
}

#[cfg(any(feature = "notifications", feature = "all"))]
pub fn send(msg: String) -> Option<bool> {
    match libnotify::Notification::new("mumd", Some(msg.as_str()), None).show() {
        Ok(_) => Some(true),
        Err(_) => {
            log::debug!("Unable to send notification");
            Some(false)
        }
    }
}

#[cfg(not(any(feature = "notifications", feature = "all")))]
pub fn send(_: String) -> Option<bool> {
    None
}
