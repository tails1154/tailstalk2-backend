//! Delete an account.
//! POST /account/delete
use revolt_config::config;
use revolt_database::{Account, Database, ValidatedTicket};
use revolt_result::Result;
use rocket::serde::json::Json;
use rocket::State;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteAccountResponse {
    /// Whether Stripe cancellation failed before account deletion continued.
    pub stripe_cancellation_failed: bool,
}
/// # Delete Account
///
/// Request to have an account deleted.
///
/// Stripe cancellation is best-effort: account deletion continues even when
/// Stripe is unavailable, but the response tells the client to contact
/// payments support so the subscription can be canceled manually.
#[openapi(tag = "Account")]
#[post("/delete")]
pub async fn delete_account(
    db: &State<Database>,
    mut account: Account,
    _ticket: ValidatedTicket,
) -> Result<Json<DeleteAccountResponse>> {
    let mut stripe_cancellation_failed = false;
    if let Some(subscription) = account.subscription.as_ref() {
        if subscription.provider == "stripe" {
            let subscription_id = subscription.provider_subscription_id.as_deref();
            let settings = config().await;

            match subscription_id {
                Some(subscription_id)
                    if settings.api.stripe.enabled && !settings.api.stripe.secret_key.is_empty() =>
                {
                    let response = reqwest::Client::new()
                        .delete(format!(
                            "https://api.stripe.com/v1/subscriptions/{subscription_id}"
                        ))
                        .basic_auth(&settings.api.stripe.secret_key, None::<&str>)
                        .send()
                        .await;

                    match response {
                        Ok(response) if response.status().is_success() => {}
                        Ok(response) => {
                            stripe_cancellation_failed = true;
                            log::error!(
                                "Stripe cancellation during account deletion failed with {}",
                                response.status()
                            );
                        }
                        Err(error) => {
                            stripe_cancellation_failed = true;
                            log::error!(
                                "Stripe cancellation during account deletion failed: {error}"
                            );
                        }
                    }
                }
                _ => {
                    stripe_cancellation_failed = true;
                    log::error!(
                        "Stripe cancellation during account deletion could not run for account {}",
                        account.id
                    );
                }
            }

            // The account is being deleted regardless of Stripe's response;
            // do not leave a local entitlement active during the deletion
            // grace period.
            account.subscription = None;
            account.save(db).await?;
        }
    }

    account
        .start_account_deletion(db)
        .await
        .map(|_| Json(DeleteAccountResponse { stripe_cancellation_failed }))
}

#[cfg(test)]
mod tests {
    use crate::{rocket, util::test::TestHarness};
    use revolt_database::MFATicket;
    use rocket::http::{ContentType, Header, Status};

    #[rocket::async_test]
    async fn success() {
        let harness = TestHarness::new().await;
        let (mut account, session, _) = harness.new_user().await;

        account.email = "delete_account@smtp.test".to_string();
        account.save(&harness.db).await.unwrap();

        let ticket = MFATicket::new(account.id.to_string(), true);
        ticket.save(&harness.db).await.unwrap();

        let res = harness.client
            .post("/auth/account/delete")
            .header(Header::new("X-Session-Token", session.token))
            .header(Header::new("X-MFA-Ticket", ticket.token))
            .dispatch()
            .await;

        assert_eq!(res.status(), Status::NoContent);

        let (_, code) = harness.assert_email("delete_account@smtp.test").await;
        let res = harness.client
            .put("/auth/account/delete")
            .header(ContentType::JSON)
            .body(
                json!({
                    "token": code
                })
                .to_string(),
            )
            .dispatch()
            .await;

        assert_eq!(res.status(), Status::NoContent);
    }
}
