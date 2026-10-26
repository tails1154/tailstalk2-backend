//! Resume a Stripe subscription that was scheduled to cancel.

use revolt_config::config;
use revolt_database::{Account, Database};
use revolt_result::{create_error, Result};
use rocket::State;
use rocket_empty::EmptyResponse;

/// Resume the current Supporter subscription before the billing period ends.
#[openapi(tag = "Account")]
#[post("/stripe/resume")]
pub async fn stripe_resume(
    db: &State<Database>,
    mut account: Account,
) -> Result<EmptyResponse> {
    let settings = config().await;
    let stripe = &settings.api.stripe;
    let subscription_id = account
        .subscription
        .as_ref()
        .and_then(|subscription| subscription.provider_subscription_id.clone())
        .ok_or_else(|| create_error!(NotFound))?;

    if !stripe.enabled || stripe.secret_key.is_empty() {
        return Err(create_error!(FeatureDisabled { feature: "Stripe subscriptions".to_owned() }));
    }

    let response = reqwest::Client::new()
        .post(format!("https://api.stripe.com/v1/subscriptions/{subscription_id}"))
        .basic_auth(&stripe.secret_key, None::<&str>)
        .form(&[("cancel_at_period_end", "false")])
        .send()
        .await
        .map_err(|_| create_error!(InternalError))?;

    if !response.status().is_success() {
        log::error!("Stripe subscription resume failed with {}", response.status());
        return Err(create_error!(InternalError));
    }

    account.subscription.as_mut().expect("subscription checked above").cancel_at_period_end = false;
    account.save(db).await?;
    Ok(EmptyResponse)
}
