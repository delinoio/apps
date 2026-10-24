// SPDX-License-Identifier: FSL-1.1-ALv2
use crate::Fault;
use buffa::Enumeration;
use connectrpc::ErrorCode;
use delino_protocol::perfine::v1::ErrorReason;

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
        ErrorCode::Unavailable => ErrorReason::AuthorityUnavailable,
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
        ] {
            let code = fault.code;
            let fault = domain(fault, expected);
            let detail = delino_protocol::perfine::v1::ErrorDetail {
                reason: reason(&fault).into(),
                ..Default::default()
            };
            assert_eq!(detail.reason, expected);
            assert_eq!(fault.code, code);
        }
        assert_eq!(reason(&Fault::exists()), ErrorReason::InvalidInput);
        assert_eq!(
            reason(&Fault::precondition("missing evidence")),
            ErrorReason::InvalidInput
        );
        assert_eq!(reason(&Fault::missing()), ErrorReason::NotFound);
    }
}
