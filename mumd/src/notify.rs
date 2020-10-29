use log::*;

pub fn init() {
    libnotify::init("mumd").unwrap();
}

pub fn send(msg: String) -> bool {
    match libnotify::Notification::new(
        "mumd",
        Some(msg.as_str()),
        None,
    ).show() {
        Ok(_) => { true }
        Err(_) => {
            debug!("Unable to send notification");
            false
        }
    }
}
