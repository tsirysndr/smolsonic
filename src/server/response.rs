use actix_web::HttpResponse;
use serde_json::{json, Value};

pub const API_VERSION: &str = "1.16.1";
pub const SERVER_TYPE: &str = "smolsonic";

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
    HttpResponse::Ok().json(json!({ "subsonic-response": body }))
}

pub fn error_json(code: u32, message: &str) -> HttpResponse {
    let body = json!({
        "status": "failed",
        "version": API_VERSION,
        "type": SERVER_TYPE,
        "error": { "code": code, "message": message }
    });
    HttpResponse::Ok().json(json!({ "subsonic-response": body }))
}
