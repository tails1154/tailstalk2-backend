//! Cancel a Stripe subscription at the end of the current billing period.

use revolt_config::config;
use revolt_database::{Account, Database};
use revolt_result::{create_error, Result};
use rocket::State;
use rocket_empty::EmptyResponse;

/// Schedule the current Supporter subscription to cancel at period end.
#[openapi(tag = "Account")]
#[post("/stripe/cancel")]
pub async fn stripe_cancel(
    db: &State<Database>,
    mut account: Account,
) -> Result<EmptyResponse> {
    let settings = config().await;
    let stripe = &settings.api.stripe;
    let subscription = account
        .subscription
        .as_mut()
        .ok_or_else(|| create_error!(NotFound))?;
    let subscription_id = subscription
        .provider_subscription_id
        .clone()
        .ok_or_else(|| create_error!(NotFound))?;

    if !stripe.enabled || stripe.secret_key.is_empty() {
        return Err(create_error!(FeatureDisabled { feature: "Stripe subscriptions".to_owned() }));
    }

    let response = reqwest::Client::new()
        .post(format!("https://api.stripe.com/v1/subscriptions/{subscription_id}"))
        .basic_auth(&stripe.secret_key, None::<&str>)
        .form(&[("cancel_at_period_end", "true")])
        .send()
        .await
        .map_err(|_| create_error!(InternalError))?;

    if !response.status().is_success() {
        log::error!("Stripe subscription cancellation failed with {}", response.status());
        return Err(create_error!(InternalError));
    }

    subscription.cancel_at_period_end = true;
    account.save(db).await?;
    Ok(EmptyResponse)
}
