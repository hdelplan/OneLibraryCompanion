//! Lossless, opt-in HTTP waveform packing; stored analysis and CDJ files stay native.
use serde_json::{Value, json};
pub(crate) fn pack(response: &mut Value) {
    response
        .as_object_mut()
        .map(|o| o.remove("beatGridSeconds"));
    for field in ["detail", "preview"] {
        let Some(wave) = response.get_mut("analysis").and_then(|a| a.get_mut(field)) else {
            continue;
        };
        if !matches!(wave["tag"].as_str(), Some("PWV6" | "PWV7")) {
            continue;
        }
        let Some(raw) = wave["rawColumns"].as_array() else {
            continue;
        };
        if wave["columns"].as_array().map(|c| c.len()) != Some(raw.len()) {
            continue;
        }
        let mut packed = String::with_capacity(raw.len() * 6);
        const HEX: &[u8] = b"0123456789abcdef";
        let mut valid = true;
        for column in raw {
            let Some(bytes) = column.as_array().filter(|a| a.len() == 3) else {
                valid = false;
                break;
            };
            for byte in bytes {
                let Some(byte) = byte.as_u64().filter(|b| *b <= 255) else {
                    valid = false;
                    break;
                };
                packed.push(HEX[(byte >> 4) as usize] as char);
                packed.push(HEX[(byte & 15) as usize] as char);
            }
            if !valid {
                break;
            }
        }
        if valid {
            let object = wave.as_object_mut().unwrap();
            object.remove("columns");
            object.remove("rawColumns");
            object.insert("packedColumns".into(), json!(packed));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_native_bytes_and_metadata_without_duplicate_arrays() {
        let mut response = json!({"beatGridSeconds":[0.],"key":"track","analysis":{"beats":[{"time":0.}],"track":{"title":"test"},"detail":{"tag":"PWV7","normalization":127,"samplesPerSecond":150,"rawColumns":[[0,16,127],[12,34,56]],"columns":[{},{}]},"preview":null}});
        pack(&mut response);
        assert_eq!(
            response["analysis"]["detail"]["packedColumns"],
            "00107f0c2238"
        );
        assert!(response["analysis"]["detail"].get("columns").is_none());
        assert_eq!(response["analysis"]["track"]["title"], "test");
        assert_eq!(response["analysis"]["beats"][0]["time"], 0.);
    }
    #[test]
    fn invalid_or_unknown_waveforms_keep_original_representation() {
        for tag in ["unknown", "PWV7"] {
            let mut v =
                json!({"analysis":{"detail":{"tag":tag,"rawColumns":[[0,256,0]],"columns":[{}]}}});
            let original = v.clone();
            pack(&mut v);
            assert_eq!(v, original);
        }
    }
}
