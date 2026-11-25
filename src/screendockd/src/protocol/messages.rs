//! Exact bytes of every host message on MI_00. Heartbeats live in `heartbeat.rs`.
//!
//! Everything except the heartbeat is compact JSON with alphabetical keys, ending in one
//! newline that is counted in the length field.

use super::framing::PictureSize;
pub use super::types::{
    EDID_ACK, EDID_PASSTHROUGH_ID, MACHINE_NAME_MAX, MOUSE_ICON_CHANGE, PLATFORM, PRODUCT_TYPE,
    PROJECTION_STOPPED,
};

/// Uppercases a hostname and truncates it to 15 characters, as the vendor driver does for
/// `platform-machine-name`.
pub fn machine_name_from_hostname(hostname: &str) -> String {
    hostname
        .trim()
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_control())
        .take(MACHINE_NAME_MAX)
        .collect()
}

/// `app-launched` request.
pub fn app_launched(id: u64, machine_name: &str, size: PictureSize) -> Vec<u8> {
    // `to_string` on a `str` value cannot fail and escapes quotes and backslashes.
    let name = serde_json::Value::from(machine_name).to_string();
    format!(
        "{{\"id\":{id},\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{{\"platform\":\"{PLATFORM}\",\"platform-machine-name\":{name},\"product-type\":\"{PRODUCT_TYPE}\",\"res-height\":{},\"res-width\":{}}}}}\n",
        size.height.0, size.width.0
    )
    .into_bytes()
}

/// `notify-projection-started` request.
pub fn projection_started(id: u64) -> Vec<u8> {
    format!("{{\"id\":{id},\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}}\n")
        .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::framing::MessageBuilder;
    use crate::protocol::heartbeat::{heartbeat_message, SessionId};

    fn size() -> PictureSize {
        PictureSize::new(1280, 720)
    }

    #[test]
    fn first_report_bytes_match_the_reference_prototype() {
        let hex =
            |report: &[u8]| -> String { report[..64].iter().map(|b| format!("{b:02x}")).collect() };
        let id = SessionId::new("25282281").unwrap();
        assert_eq!(
            hex(&heartbeat_message(&id)[0]),
            "010000004e00000000000000000000007b226a736f6e727063223a22322e30222c20226d6574686f64223a2022686561727462656174222c2022706172616d73"
        );
        let launched = app_launched(1, "VERMELHO", size());
        let chunks = MessageBuilder::control().payload(&launched).build();
        assert_eq!(
            hex(&chunks[0]),
            "01000000b600000000000000000000007b226964223a312c226a736f6e727063223a22322e30222c226d6574686f64223a226170702d6c61756e63686564222c"
        );
    }

    #[test]
    fn message_bytes_are_exact() {
        assert_eq!(
            app_launched(1, "AZUL", PictureSize::new(1920, 1080)),
            b"{\"id\":1,\"jsonrpc\":\"2.0\",\"method\":\"app-launched\",\"params\":{\"platform\":\"windows\",\"platform-machine-name\":\"AZUL\",\"product-type\":\"QuattroTx CT10\",\"res-height\":1080,\"res-width\":1920}}\n"
        );
        assert_eq!(
            app_launched(3, "AZUL", size()).len(),
            0xb2,
            "captured length of the 1280x720 request"
        );
        assert_eq!(
            projection_started(4),
            b"{\"id\":4,\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-started\"}\n"
        );
        assert_eq!(
            PROJECTION_STOPPED,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notify-projection-stopped\"}\n"
        );
        assert_eq!(
            MOUSE_ICON_CHANGE,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"app_tx_mouse_icon_change\",\"params\":{\"cursor_base64_data\":\"\"}}\n"
        );
        assert_eq!(
            EDID_ACK,
            b"{\"jsonrpc\":\"2.0\",\"method\":\"common.data_passthrough\",\"params\":{\"data\":\"AQ==\",\"id\":103,\"size\":1}}\n"
        );
    }

    #[test]
    fn machine_name_is_uppercase_and_truncated_to_15() {
        assert_eq!(machine_name_from_hostname("vermelho\n"), "VERMELHO");
        assert_eq!(
            machine_name_from_hostname("a-very-long-hostname-indeed"),
            "A-VERY-LONG-HOS"
        );
        let quoted = app_launched(1, "A\"B", size());
        assert!(serde_json::from_slice::<serde_json::Value>(&quoted).is_ok());
    }
}
