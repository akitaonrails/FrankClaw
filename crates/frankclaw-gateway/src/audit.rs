use serde_json::Value;

#[expect(clippy::needless_pass_by_value, reason = "Value is consumed by tracing macro formatting")]
pub fn log_event(action: &str, outcome: &str, details: Value) {
    tracing::info!(
        target: "frankclaw_audit",
        action,
        outcome,
        details = %details,
        "security audit event"
    );
}

#[expect(clippy::needless_pass_by_value, reason = "Value is consumed by tracing macro formatting")]
pub fn log_failure(action: &str, details: Value) {
    tracing::warn!(
        target: "frankclaw_audit",
        action,
        outcome = "failure",
        details = %details,
        "security audit event"
    );
}
