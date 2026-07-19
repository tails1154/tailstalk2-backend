use revolt_result::{create_error, Error};
use rocket::{
    http::Status,
    request::{FromRequest, Outcome, Request},
};

/// Newtype for an audit log reason.
///
/// Extracts the reason from the `X-Audit-Log-Reason` header if provided.
pub struct AuditLogReason(pub Option<String>);

#[async_trait]
impl<'r> FromRequest<'r> for AuditLogReason {
    type Error = Error;

    async fn from_request(req: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let reason = req.headers().get_one("x-audit-log-reason");

        if reason.is_some_and(|str| str.len() > 512) {
            return Outcome::Error((Status::BadRequest, create_error!(HeaderTooLarge)));
        };

        Outcome::Success(Self(reason.map(|str| str.to_string())))
    }
}

impl From<AuditLogReason> for Option<String> {
    fn from(value: AuditLogReason) -> Self {
        value.0
    }
}