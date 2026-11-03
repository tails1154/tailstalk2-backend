//! Request a generic cash payment review for TailsTalk 2 Premium.

use iso8601_timestamp::Timestamp;
use revolt_database::{Account, CashPaymentRequest, Database};
use revolt_result::Result;
use rocket::State;
use rocket_empty::EmptyResponse;

/// Create a pending cash payment request for the current account.
#[openapi(tag = "Account")]
#[post("/cash-payment")]
pub async fn cash_payment(db: &State<Database>, mut account: Account) -> Result<EmptyResponse> {
    if account
        .cash_payment_request
        .as_ref()
        .is_some_and(|request| request.status == "pending")
    {
        return Ok(EmptyResponse);
    }

    account.cash_payment_request = Some(CashPaymentRequest {
        status: "pending".to_owned(),
        requested_at: Timestamp::now_utc(),
    });
    account.save(db).await?;
    Ok(EmptyResponse)
}
