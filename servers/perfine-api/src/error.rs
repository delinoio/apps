// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Fault;
use buffa::Enumeration;
use connectrpc::ErrorCode;
use delino_protocol::perfine::v1::ErrorReason;
use server_common::error::FaultSource;

pub(crate) fn unsupported_major(value: &serde_json::Value, key: &str) -> bool {
    value[key]
        .as_u64()
        .or_else(|| value[key].as_str().and_then(|text| text.parse().ok()))
        .is_some_and(|version| version > 1)
}

pub(crate) fn domain(mut fault: Fault, reason: ErrorReason) -> Fault {
    // Shared faults use nonnegative reasons in a different protocol namespace.
    // Tag Perfine reasons negatively until the shared fault carries typed domains.
    fault.reason = -(reason as i32);
    fault
}

pub(crate) fn reason(fault: &Fault) -> ErrorReason {
    if fault.reason < 0
        && let Some(reason) = fault.reason.checked_neg().and_then(ErrorReason::from_i32)
    {
        return reason;
    }
    match fault.code {
        ErrorCode::Unauthenticated => ErrorReason::Unauthenticated,
        ErrorCode::PermissionDenied => ErrorReason::PermissionDenied,
        ErrorCode::Unavailable => match fault.source {
            FaultSource::Persistence | FaultSource::GitHub => ErrorReason::TransientFailure,
            FaultSource::Application => ErrorReason::AuthorityUnavailable,
        },
        ErrorCode::NotFound => ErrorReason::NotFound,
        ErrorCode::ResourceExhausted => ErrorReason::LimitExceeded,
        _ => ErrorReason::InvalidInput,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_domain_failures_preserve_transport_codes_and_recovery_reasons() {
        for (fault, expected) in [
            (Fault::conflict(), ErrorReason::RequestIdConflict),
            (
                Fault::precondition("stale binding"),
                ErrorReason::ApprovalStale,
            ),
            (
                Fault::invalid("unsupported version"),
                ErrorReason::UnsupportedVersion,
            ),
            (
                Fault::precondition("experiment deleted"),
                ErrorReason::Deleted,
            ),
            (
                Fault::precondition("connection inactive"),
                ErrorReason::ConnectionInactive,
            ),
            (Fault::exists(), ErrorReason::RepositoryAlreadyConnected),
            (Fault::denied(), ErrorReason::RepositoryAdminRequired),
            (
                Fault::precondition("fingerprint mismatch"),
                ErrorReason::FingerprintMismatch,
            ),
            (
                Fault::precondition("evidence incomplete"),
                ErrorReason::EvidenceIncomplete,
            ),
            (
                Fault::precondition("artifact ineligible"),
                ErrorReason::ArtifactIneligible,
            ),
            (
                Fault::precondition("consent required"),
                ErrorReason::ConsentRequired,
            ),
            (
                Fault::precondition("secret detected"),
                ErrorReason::SecretDetected,
            ),
            (
                Fault::precondition("publication uncertain"),
                ErrorReason::PublicationUncertain,
            ),
            (
                Fault::precondition("cloud unavailable"),
                ErrorReason::CloudUnavailable,
            ),
            (Fault::unavailable(), ErrorReason::TransientFailure),
        ] {
            let code = fault.code;
            let message = fault.message.clone();
            let fault = domain(fault, expected);
            let detail = delino_protocol::perfine::v1::ErrorDetail {
                reason: reason(&fault).into(),
                ..Default::default()
            };
            assert_eq!(detail.reason, expected);
            assert_eq!(fault.code, code);
            assert_eq!(fault.message, message);
        }
        assert_eq!(reason(&Fault::exists()), ErrorReason::InvalidInput);
        assert_eq!(
            reason(&Fault::precondition("missing evidence")),
            ErrorReason::InvalidInput
        );
        assert_eq!(reason(&Fault::missing()), ErrorReason::NotFound);
        assert!(unsupported_major(
            &serde_json::json!({"schemaVersion":2}),
            "schemaVersion"
        ));
        assert!(!unsupported_major(
            &serde_json::json!({"schemaVersion":1}),
            "schemaVersion"
        ));
        assert!(!unsupported_major(&serde_json::json!({}), "schemaVersion"));
    }

    #[test]
    fn unavailable_reason_uses_provider_provenance() {
        let database = Fault::from(sqlx::Error::PoolClosed);
        assert_eq!(database.code, ErrorCode::Unavailable);
        assert_eq!(reason(&database), ErrorReason::TransientFailure);
        let github = crate::github::GitHubCallError::Response {
            status: reqwest::StatusCode::BAD_GATEWAY,
            rate_limited: false,
        }
        .into_fault();
        assert_eq!(github.code, ErrorCode::Unavailable);
        assert_eq!(github.source, FaultSource::GitHub);
        assert_eq!(reason(&github), ErrorReason::TransientFailure);
        let transport = crate::github::GitHubCallError::Fault(Fault::unavailable()).into_fault();
        assert_eq!(transport.source, FaultSource::GitHub);
        assert_eq!(reason(&transport), ErrorReason::TransientFailure);
        assert_eq!(
            reason(&Fault::unavailable()),
            ErrorReason::AuthorityUnavailable
        );
    }
}
