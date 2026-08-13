use std::time::Duration;

use axum::http::StatusCode;

use crate::domain::InferenceFailureReason;

pub const SSE_PREFLIGHT_LIMIT: usize = 256 * 1024;
pub const FIRST_MEANINGFUL_OUTPUT_TIMEOUT: Duration = Duration::from_mins(5);
pub const CAPACITY_COMPATIBILITY_MESSAGE: &str =
    "Selected model is at capacity. Please try a different model.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportFailure {
    FastConnection,
    FastRequest,
    FastRead,
    ElapsedTimeout,
    InvalidEncoding,
    ResponseTooLarge,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailurePolicy {
    ForwardImmediately,
    ReturnImmediately,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassifiedFailure {
    pub policy: FailurePolicy,
    pub category: &'static str,
    pub reason: Option<InferenceFailureReason>,
}

pub fn classify_transport(failure: TransportFailure) -> ClassifiedFailure {
    match failure {
        TransportFailure::FastConnection => ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_connection_failed",
            reason: Some(InferenceFailureReason::Connection),
        },
        TransportFailure::FastRequest => ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_request_failed",
            reason: Some(InferenceFailureReason::Connection),
        },
        TransportFailure::FastRead => ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_read_failed",
            reason: Some(InferenceFailureReason::Connection),
        },
        TransportFailure::ElapsedTimeout => ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_timeout",
            reason: Some(InferenceFailureReason::Timeout),
        },
        TransportFailure::InvalidEncoding => ClassifiedFailure {
            policy: FailurePolicy::ReturnImmediately,
            category: "upstream_invalid_encoding",
            reason: None,
        },
        TransportFailure::ResponseTooLarge => ClassifiedFailure {
            policy: FailurePolicy::ReturnImmediately,
            category: "upstream_response_too_large",
            reason: None,
        },
    }
}

pub fn classify_http(status: StatusCode, safe_error_code: Option<&str>) -> ClassifiedFailure {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_rate_limited",
            reason: Some(InferenceFailureReason::RateLimit),
        };
    }
    if let Some((category, reason)) = classify_account_code(safe_error_code) {
        return ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category,
            reason: Some(reason),
        };
    }
    if matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    ) {
        return ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: if status == StatusCode::REQUEST_TIMEOUT {
                "upstream_timeout"
            } else {
                "upstream_http_status"
            },
            reason: Some(if status == StatusCode::REQUEST_TIMEOUT {
                InferenceFailureReason::Timeout
            } else {
                InferenceFailureReason::Service
            }),
        };
    }
    if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED
    ) {
        return ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_auth_failed",
            reason: Some(InferenceFailureReason::Authentication),
        };
    }
    if status.is_server_error() {
        return ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_http_status",
            reason: Some(InferenceFailureReason::Service),
        };
    }
    if status == StatusCode::FORBIDDEN {
        return ClassifiedFailure {
            policy: FailurePolicy::ReturnImmediately,
            category: "upstream_access_denied",
            reason: Some(InferenceFailureReason::AccessDenied),
        };
    }
    ClassifiedFailure {
        policy: FailurePolicy::ReturnImmediately,
        category: if matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED
        ) {
            "upstream_auth_failed"
        } else {
            "upstream_http_status"
        },
        reason: matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED
        )
        .then_some(InferenceFailureReason::Authentication),
    }
}

pub fn classify_semantic(
    http_status: StatusCode,
    status: Option<&str>,
    safe_error_code: Option<&str>,
) -> ClassifiedFailure {
    if http_status != StatusCode::OK || status != Some("failed") {
        return ClassifiedFailure {
            policy: FailurePolicy::ReturnImmediately,
            category: "upstream_semantic_failure",
            reason: None,
        };
    }
    if safe_error_code == Some("server_overloaded") {
        ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_overloaded",
            reason: Some(InferenceFailureReason::Service),
        }
    } else if matches!(
        safe_error_code,
        Some("model_not_found" | "model_unavailable" | "unsupported_model")
    ) {
        ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category: "upstream_model_unavailable",
            reason: Some(InferenceFailureReason::Service),
        }
    } else if let Some((category, reason)) = classify_account_code(safe_error_code) {
        ClassifiedFailure {
            policy: FailurePolicy::ForwardImmediately,
            category,
            reason: Some(reason),
        }
    } else {
        ClassifiedFailure {
            policy: FailurePolicy::ReturnImmediately,
            category: "upstream_semantic_failure",
            reason: None,
        }
    }
}

#[must_use]
pub fn normalize_semantic_error_code(
    code: Option<&str>,
    codex_error_info: Option<&str>,
    message: Option<&str>,
) -> Option<String> {
    if codex_error_info == Some("server_overloaded")
        || (code == Some("server_error") && message == Some(CAPACITY_COMPATIBILITY_MESSAGE))
    {
        Some("server_overloaded".to_owned())
    } else {
        code.map(str::to_owned)
    }
}

fn classify_account_code(
    safe_error_code: Option<&str>,
) -> Option<(&'static str, InferenceFailureReason)> {
    match safe_error_code? {
        "invalid_api_key" | "authentication_error" => {
            Some(("invalid_api_key", InferenceFailureReason::InvalidKey))
        }
        "insufficient_quota" | "credits_exhausted" => Some((
            "insufficient_quota",
            InferenceFailureReason::InsufficientQuota,
        )),
        "billing_hard_limit_reached" => Some((
            "billing_hard_limit_reached",
            InferenceFailureReason::BillingLimit,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifier_forwards_bare_auth_and_remains_exact_for_account_codes() {
        assert_eq!(
            classify_http(StatusCode::UNAUTHORIZED, None).policy,
            FailurePolicy::ForwardImmediately
        );
        assert_eq!(
            classify_http(StatusCode::FORBIDDEN, Some("unknown_code")).policy,
            FailurePolicy::ReturnImmediately
        );
        assert_eq!(
            classify_http(StatusCode::FORBIDDEN, Some("unknown_code")),
            ClassifiedFailure {
                policy: FailurePolicy::ReturnImmediately,
                category: "upstream_access_denied",
                reason: Some(InferenceFailureReason::AccessDenied),
            }
        );
        assert_eq!(
            classify_http(StatusCode::UNAUTHORIZED, Some("invalid_api_key")).policy,
            FailurePolicy::ForwardImmediately
        );
        assert_eq!(
            classify_http(StatusCode::PAYMENT_REQUIRED, Some("insufficient_quota")).reason,
            Some(InferenceFailureReason::InsufficientQuota)
        );
    }

    #[test]
    fn classifier_separates_transient_timeout_rate_limit_and_excluded_errors() {
        for (case, actual, expected) in [
            (
                "fast connection",
                classify_transport(TransportFailure::FastConnection).policy,
                FailurePolicy::ForwardImmediately,
            ),
            (
                "elapsed timeout",
                classify_transport(TransportFailure::ElapsedTimeout).policy,
                FailurePolicy::ForwardImmediately,
            ),
            (
                "http 5xx",
                classify_http(StatusCode::INTERNAL_SERVER_ERROR, None).policy,
                FailurePolicy::ForwardImmediately,
            ),
            (
                "http 429",
                classify_http(StatusCode::TOO_MANY_REQUESTS, None).policy,
                FailurePolicy::ForwardImmediately,
            ),
            (
                "excluded http 4xx",
                classify_http(StatusCode::BAD_REQUEST, None).policy,
                FailurePolicy::ReturnImmediately,
            ),
        ] {
            assert_eq!(actual, expected, "{case}");
        }
        assert_eq!(
            classify_transport(TransportFailure::FastConnection).category,
            "upstream_connection_failed"
        );
        assert_eq!(
            classify_transport(TransportFailure::FastRequest).category,
            "upstream_request_failed"
        );
        assert_eq!(
            classify_transport(TransportFailure::FastRead).category,
            "upstream_read_failed"
        );
    }

    #[test]
    fn semantic_classifier_forwards_only_exact_overload_code() {
        assert_eq!(
            classify_semantic(StatusCode::OK, Some("failed"), Some("server_overloaded"),),
            ClassifiedFailure {
                policy: FailurePolicy::ForwardImmediately,
                category: "upstream_overloaded",
                reason: Some(InferenceFailureReason::Service),
            }
        );
        for code in [
            None,
            Some("server_error"),
            Some("server_overloaded_extra"),
            Some("capacity"),
        ] {
            assert_eq!(
                classify_semantic(StatusCode::OK, Some("failed"), code),
                ClassifiedFailure {
                    policy: FailurePolicy::ReturnImmediately,
                    category: "upstream_semantic_failure",
                    reason: None,
                },
                "semantic code {code:?}"
            );
        }
        for status in [None, Some("cancelled"), Some("incomplete")] {
            assert_eq!(
                classify_semantic(StatusCode::OK, status, Some("server_overloaded")),
                ClassifiedFailure {
                    policy: FailurePolicy::ReturnImmediately,
                    category: "upstream_semantic_failure",
                    reason: None,
                },
                "semantic status {status:?}"
            );
        }
    }

    #[test]
    fn classifier_immediately_forwards_high_confidence_http_failures() {
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
            StatusCode::UNAUTHORIZED,
            StatusCode::PAYMENT_REQUIRED,
        ] {
            assert_eq!(
                classify_http(status, None).policy,
                FailurePolicy::ForwardImmediately,
                "status {status}"
            );
        }
        assert_eq!(
            classify_http(StatusCode::INTERNAL_SERVER_ERROR, None).policy,
            FailurePolicy::ForwardImmediately
        );
        assert_eq!(
            classify_http(StatusCode::FORBIDDEN, None).policy,
            FailurePolicy::ReturnImmediately
        );
    }

    #[test]
    fn semantic_classifier_requires_exact_http_and_failed_status() {
        for code in ["model_not_found", "model_unavailable", "unsupported_model"] {
            assert_eq!(
                classify_semantic(StatusCode::OK, Some("failed"), Some(code)).policy,
                FailurePolicy::ForwardImmediately,
                "code {code}"
            );
        }
        for status in [StatusCode::CREATED, StatusCode::NO_CONTENT] {
            assert_eq!(
                classify_semantic(status, Some("failed"), Some("model_unavailable")).policy,
                FailurePolicy::ReturnImmediately,
                "status {status}"
            );
        }
    }

    #[test]
    fn compatibility_capacity_message_is_exact_and_dropped_after_normalization() {
        assert_eq!(
            normalize_semantic_error_code(
                Some("server_error"),
                None,
                Some(CAPACITY_COMPATIBILITY_MESSAGE),
            )
            .as_deref(),
            Some("server_overloaded")
        );
        for message in [
            "Selected model is at capacity. Please try a different model",
            "selected model is at capacity. please try a different model.",
            "Selected model is at capacity. Please try a different model. ",
        ] {
            assert_eq!(
                normalize_semantic_error_code(Some("server_error"), None, Some(message)).as_deref(),
                Some("server_error"),
                "message {message:?}"
            );
        }
    }
}
