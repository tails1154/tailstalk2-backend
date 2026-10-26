//! Create a Stripe Checkout Session for the current account.

use revolt_config::config;
use revolt_database::Account;
use revolt_result::{create_error, Result};
use rocket::serde::json::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct CheckoutResponse {
    url: String,
}

/// Create a hosted Stripe subscription checkout session.
#[openapi(tag = "Account")]
#[post("/stripe/checkout")]
pub async fn stripe_checkout(account: Account) -> Result<Json<CheckoutResponse>> {
    let settings = config().await;
    let stripe = &settings.api.stripe;
    if !stripe.enabled || stripe.secret_key.is_empty() || stripe.supporter_price_id.is_empty() {
        return Err(create_error!(FeatureDisabled { feature: "Stripe subscriptions".to_owned() }));
    }

    let response = reqwest::Client::new()
        .post("https://api.stripe.com/v1/checkout/sessions")
        .basic_auth(&stripe.secret_key, None::<&str>)
        .form(&[
            ("mode", "subscription"),
            ("line_items[0][price]", stripe.supporter_price_id.as_str()),
            ("line_items[0][quantity]", "1"),
            ("customer_email", account.email.as_str()),
            ("metadata[tailstalk_account_id]", account.id.as_str()),
            ("subscription_data[metadata][tailstalk_account_id]", account.id.as_str()),
            ("success_url", stripe.success_url.as_str()),
            ("cancel_url", stripe.cancel_url.as_str()),
        ])
        .send()
        .await
        .map_err(|_| create_error!(InternalError))?;

    if !response.status().is_success() {
        log::error!("Stripe Checkout session creation failed with {}", response.status());
        return Err(create_error!(InternalError));
    }
    let session = response
        .json::<CheckoutResponse>()
        .await
        .map_err(|_| create_error!(InternalError))?;
    Ok(Json(session))
}
