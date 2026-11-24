//! Prints the first 64 bytes of the MI_00 heartbeat and app-launched reports as hex, for
//! byte comparison against the reference prototype.

use screendockd::framing::{MessageBuilder, PictureSize};
use screendockd::heartbeat::{heartbeat_message, SessionId};
use screendockd::session::messages;

fn hex(report: &[u8]) -> String {
    report[..64].iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let id = SessionId::new("25282281").expect("valid session id");
    println!("heartbeat {}", hex(&heartbeat_message(&id)[0]));
    let launched = messages::app_launched(1, "VERMELHO", PictureSize::new(1280, 720));
    let chunks = MessageBuilder::control().payload(&launched).build();
    println!("app-launched {}", hex(&chunks[0]));
}
