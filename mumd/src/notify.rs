pub fn init() {
    #[cfg(feature = "libnotify")]
    libnotify::init("mumd").unwrap();
}

#[cfg(feature = "libnotify")]
pub fn send(msg: String) -> Option<bool> {
    match libnotify::Notification::new("mumd", Some(msg.as_str()), None).show() {
        Ok(_) => Some(true),
        Err(_) => {
            log::debug!("Unable to send notification");
            Some(false)
        }
    }
}

#[cfg(not(feature = "libnotify"))]
pub fn send(_: String) -> Option<bool> {
    None
}
