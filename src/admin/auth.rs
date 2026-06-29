use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use base64::Engine;
use tower_http::validate_request::{ValidateRequest, ValidateRequestHeaderLayer};

use crate::config::Config;

#[derive(Clone)]
pub struct BasicAuth {
    // Pre-encoded expected "Basic <base64(user:pass)>" value
    expected: String,
}

impl BasicAuth {
    fn new(username: &str, password: &str) -> Self {
        use base64::engine::general_purpose::STANDARD;
        let encoded = STANDARD.encode(format!("{username}:{password}"));
        Self {
            expected: format!("Basic {encoded}"),
        }
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

        if auth_header == self.expected {
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
