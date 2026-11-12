//! mpv JSON IPC line format (pure).

use serde_json::{Value, json};

/// One line received from mpv.
#[derive(Debug, Clone, PartialEq)]
pub enum MpvIncoming {
    Reply { request_id: u64, error: String, data: Value },
    Event { event: String, name: Option<String>, data: Option<Value>, reason: Option<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("invalid JSON from mpv: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unrecognized mpv message: {0}")]
    Unknown(String),
}

pub fn parse_line(line: &str) -> Result<MpvIncoming, WireError> {
    let v: Value = serde_json::from_str(line)?;
    let str_field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    if let Some(event) = str_field("event") {
        return Ok(MpvIncoming::Event {
            event,
            name: str_field("name"),
            data: v.get("data").cloned(),
            reason: str_field("reason"),
        });
    }
    if let (Some(request_id), Some(error)) =
        (v.get("request_id").and_then(Value::as_u64), str_field("error"))
    {
        return Ok(MpvIncoming::Reply {
            request_id,
            error,
            data: v.get("data").cloned().unwrap_or(Value::Null),
        });
    }
    Err(WireError::Unknown(line.to_owned()))
}

/// Serialize one command, newline-terminated.
pub fn command_line(request_id: u64, args: &[Value]) -> String {
    let mut s = json!({ "command": args, "request_id": request_id }).to_string();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parse_reply() {
        let m = parse_line(r#"{"request_id":3,"error":"success","data":1.5}"#).unwrap();
        assert_eq!(
            m,
            MpvIncoming::Reply { request_id: 3, error: "success".into(), data: json!(1.5) }
        );
    }

    #[test]
    fn parse_reply_without_data() {
        let m = parse_line(r#"{"request_id":4,"error":"property unavailable"}"#).unwrap();
        assert_eq!(
            m,
            MpvIncoming::Reply {
                request_id: 4,
                error: "property unavailable".into(),
                data: json!(null)
            }
        );
    }

    #[test]
    fn parse_property_change() {
        let m =
            parse_line(r#"{"event":"property-change","id":1,"name":"pause","data":true}"#).unwrap();
        assert_eq!(
            m,
            MpvIncoming::Event {
                event: "property-change".into(),
                name: Some("pause".into()),
                data: Some(json!(true)),
                reason: None,
            }
        );
    }

    #[test]
    fn parse_end_file() {
        let m = parse_line(r#"{"event":"end-file","reason":"eof","playlist_entry_id":1}"#).unwrap();
        assert!(
            matches!(m, MpvIncoming::Event { ref event, reason: Some(ref r), .. } if event == "end-file" && r == "eof")
        );
    }

    #[test]
    fn parse_garbage_fails() {
        assert!(parse_line("nope").is_err());
        assert!(parse_line(r#"{"foo":1}"#).is_err());
    }

    #[test]
    fn command_line_format() {
        assert_eq!(
            command_line(7, &[json!("seek"), json!(12.5), json!("absolute+exact")]),
            "{\"command\":[\"seek\",12.5,\"absolute+exact\"],\"request_id\":7}\n"
        );
    }
}
