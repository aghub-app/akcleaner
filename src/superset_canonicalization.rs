use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CanonicalizationError {
    UnsupportedShape,
}

/// Hashes the supported upstream Superset server shape. Values outside this
/// deliberately narrow subset must remain unknown rather than be guessed.
pub(crate) fn canonical_server_sha256(value: &Value) -> Result<String, CanonicalizationError> {
    const SERVER_KEYS: &[&str] = &["args", "command", "env", "type", "url"];
    const ENV_KEYS: &[&str] = &["HOME", "PATH", "SHELL", "TMPDIR", "USER"];

    let server = value
        .as_object()
        .ok_or(CanonicalizationError::UnsupportedShape)?;
    if server
        .keys()
        .any(|key| !SERVER_KEYS.contains(&key.as_str()))
    {
        return Err(CanonicalizationError::UnsupportedShape);
    }

    let mut bytes = Vec::new();
    bytes.push(b'{');
    for (index, (key, value)) in server.iter().enumerate() {
        if index != 0 {
            bytes.push(b',');
        }
        write_json_string(key, &mut bytes);
        bytes.push(b':');
        match key.as_str() {
            "args" => {
                let args = value
                    .as_array()
                    .ok_or(CanonicalizationError::UnsupportedShape)?;
                if args.iter().any(|item| !item.is_string()) {
                    return Err(CanonicalizationError::UnsupportedShape);
                }
                write_json_value(value, &mut bytes);
            }
            "env" => {
                let env = value
                    .as_object()
                    .ok_or(CanonicalizationError::UnsupportedShape)?;
                if env
                    .iter()
                    .any(|(key, item)| !ENV_KEYS.contains(&key.as_str()) || !item.is_string())
                {
                    return Err(CanonicalizationError::UnsupportedShape);
                }
                bytes.push(b'{');
                for (env_index, (env_key, env_value)) in env.iter().enumerate() {
                    if env_index != 0 {
                        bytes.push(b',');
                    }
                    write_json_string(env_key, &mut bytes);
                    bytes.push(b':');
                    write_json_value(env_value, &mut bytes);
                }
                bytes.push(b'}');
            }
            "command" | "type" | "url" => {
                if !value.is_string() {
                    return Err(CanonicalizationError::UnsupportedShape);
                }
                write_json_value(value, &mut bytes);
            }
            _ => unreachable!("server keys were checked against the supported set"),
        }
    }
    bytes.push(b'}');
    Ok(hex(&Sha256::digest(bytes)))
}

fn write_json_string(value: &str, output: &mut Vec<u8>) {
    serde_json::to_writer(output, value).expect("writing JSON string to Vec cannot fail");
}

fn write_json_value(value: &Value, output: &mut Vec<u8>) {
    serde_json::to_writer(output, value).expect("writing JSON value to Vec cannot fail");
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
