#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use jensen_plugin_backend::Backend;

pub mod compose;
pub mod files;
pub mod interpolate;
pub mod model;
pub mod paths;
pub mod short_syntax;
pub mod source;
pub mod yaml;

/// Adds the backend's methods. There are none yet: the next changes register `analyze`, `topology`
/// and `readiness` here.
pub fn register(_backend: &mut Backend) {}

jensen_plugin_backend::export!(register);

#[cfg(test)]
mod tests {
    use jensen_plugin_backend::{Backend, Value};

    use super::register;

    #[test]
    fn an_unknown_method_answers_with_an_error() {
        let mut backend = Backend::default();
        register(&mut backend);

        let reply = backend.handle("nope", b"{}");

        let envelope: Value = serde_json::from_slice(&reply).unwrap();
        let message = envelope["error"].as_str().unwrap_or_default();
        assert!(
            message.contains("nope"),
            "expected an error envelope, got {envelope}"
        );
        assert!(envelope.get("ok").is_none(), "unexpected ok in {envelope}");
    }
}
