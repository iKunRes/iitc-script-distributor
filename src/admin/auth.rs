use argon2::{Argon2, PasswordHash, PasswordVerifier};
use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use base64::Engine;
use tower_http::validate_request::{ValidateRequest, ValidateRequestHeaderLayer};

use crate::config::Config;

#[derive(Clone)]
pub struct BasicAuth {
    username: String,
    // PHC hash string, e.g. $argon2id$v=19$...
    password_hash: String,
}

impl BasicAuth {
    fn new(username: &str, password_hash: &str) -> Self {
        Self {
            username: username.to_string(),
            password_hash: password_hash.to_string(),
        }
    }

    fn verify(&self, header_value: &str) -> bool {
        use base64::engine::general_purpose::STANDARD;

        let encoded = header_value.strip_prefix("Basic ").unwrap_or("");
        let decoded = match STANDARD.decode(encoded) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let credentials = match std::str::from_utf8(&decoded) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let (user, pass) = match credentials.split_once(':') {
            Some(pair) => pair,
            None => return false,
        };

        if user != self.username {
            return false;
        }

        let parsed_hash = match PasswordHash::new(&self.password_hash) {
            Ok(h) => h,
            Err(_) => return false,
        };

        Argon2::default()
            .verify_password(pass.as_bytes(), &parsed_hash)
            .is_ok()
    }
}

impl<B> ValidateRequest<B> for BasicAuth {
    type ResponseBody = Body;

    fn validate(&mut self, request: &mut Request<B>) -> Result<(), Response<Self::ResponseBody>> {
        let auth_header = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if self.verify(auth_header) {
            Ok(())
        } else {
            Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, r#"Basic realm="admin""#)
                .body(Body::empty())
                .unwrap())
        }
    }
}

pub fn basic_auth_layer(config: &Config) -> ValidateRequestHeaderLayer<BasicAuth> {
    ValidateRequestHeaderLayer::custom(BasicAuth::new(
        &config.admin.username,
        &config.admin.password,
    ))
}
