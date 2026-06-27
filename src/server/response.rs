use actix_web::HttpResponse;
use serde_json::{json, Value};

pub const API_VERSION: &str = "1.16.1";
pub const SERVER_TYPE: &str = "smolsonic";

// Strip JSON `null` from objects (and from inside arrays) recursively.
// OpenSubsonic clients (e.g. Firmium's Gson-based Android parser) crash on
// present-but-null scalars because `JsonElement?.asInt` doesn't short-circuit
// on the JsonNull singleton. Acts as a safety net so individual handlers don't
// have to remember to omit each optional field.
fn strip_nulls(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.retain(|_, val| !val.is_null());
            for val in map.values_mut() {
                strip_nulls(val);
            }
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                strip_nulls(item);
            }
        }
        _ => {}
    }
}

pub fn ok_json(data: Value) -> HttpResponse {
    let mut body = json!({
        "status": "ok",
        "version": API_VERSION,
        "type": SERVER_TYPE,
    });
    if let (Some(obj), Some(data_obj)) = (body.as_object_mut(), data.as_object()) {
        for (k, v) in data_obj {
            obj.insert(k.clone(), v.clone());
        }
    }
    let mut envelope = json!({ "subsonic-response": body });
    strip_nulls(&mut envelope);
    HttpResponse::Ok().json(envelope)
}

pub fn error_json(code: u32, message: &str) -> HttpResponse {
    let body = json!({
        "status": "failed",
        "version": API_VERSION,
        "type": SERVER_TYPE,
        "error": { "code": code, "message": message }
    });
    let mut envelope = json!({ "subsonic-response": body });
    strip_nulls(&mut envelope);
    HttpResponse::Ok().json(envelope)
}
