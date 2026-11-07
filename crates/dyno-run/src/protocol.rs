//! JSON lines framing for the runner protocol in `dyno_schema::runner`.
//!
//! Each message is one JSON object on one line. `serde_json` escapes newlines inside strings,
//! so a message never spans two lines.

use dyno_schema::runner::DriverMessage;
use serde::Serialize;

/// One message as a line, with its final newline.
pub fn encode_line<T: Serialize>(message: &T) -> Result<String, serde_json::Error> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    Ok(line)
}

/// One driver message from a line. Surrounding whitespace, such as the newline, is ignored.
pub fn decode_line(line: &str) -> Result<DriverMessage, serde_json::Error> {
    serde_json::from_str(line.trim())
}

#[cfg(test)]
mod tests {
    use dyno_schema::probe::ProbeEvent;
    use dyno_schema::result::DriverInfo;
    use dyno_schema::runner::{RUNNER_PROTOCOL_VERSION, RunMark};

    use super::*;

    #[test]
    fn messages_survive_a_round_trip_on_one_line_each() {
        let messages = [
            DriverMessage::Hello {
                protocol_version: RUNNER_PROTOCOL_VERSION,
                driver: DriverInfo {
                    name: "test".to_owned(),
                    version: "0".to_owned(),
                    browser: None,
                },
            },
            DriverMessage::Probe {
                event: ProbeEvent::FirstFrame { t_ms: 12.5 },
            },
            DriverMessage::Mark {
                mark: RunMark::Idle,
                t_ms: 900.0,
            },
            DriverMessage::Error {
                message: "line one\nline two".to_owned(),
                t_ms: 1_000.0,
            },
        ];
        for message in messages {
            let line = encode_line(&message).unwrap();
            assert_eq!(line.matches('\n').count(), 1, "{line}");
            assert!(line.ends_with('\n'));
            assert_eq!(decode_line(&line).unwrap(), message);
        }
    }

    #[test]
    fn messages_are_tagged_by_type() {
        let line = encode_line(&DriverMessage::Done { t_ms: 1.0 }).unwrap();
        assert_eq!(line, "{\"type\":\"done\",\"t_ms\":1.0}\n");
    }

    #[test]
    fn an_unknown_message_type_is_an_error() {
        assert!(decode_line("{\"type\":\"teleport\"}").is_err());
    }
}
