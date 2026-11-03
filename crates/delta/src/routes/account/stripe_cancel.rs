//! Cancel a Stripe subscription immediately.

use revolt_config::config;
use revolt_database::{Account, Database, PartialUser};
use revolt_models::v0::UserBadges;
use revolt_result::{create_error, Result};
use rocket::State;
use rocket::serde::json::Json;
use rocket_empty::EmptyResponse;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, JsonSchema)]
pub struct CancelSubscriptionData {
    #[serde(default)]
    pub immediate: bool,
}

/// Cancel the current TailsTalk 2 Premium subscription immediately.
#[openapi(tag = "Account")]
#[post("/stripe/cancel", data = "<data>")]
pub async fn stripe_cancel(
    db: &State<Database>,
    mut account: Account,
    data: Json<CancelSubscriptionData>,
) -> Result<EmptyResponse> {
    let immediate = data.immediate;
    let settings = config().await;
    let stripe = &settings.api.stripe;
    let subscription_id = account
        .subscription
        .as_ref()
        .and_then(|subscription| subscription.provider_subscription_id.clone());

    // Cancellation is intentionally idempotent. Cash/manual entitlements and
    // stale local records do not have a Stripe subscription ID, but they still
    // need to be removable from the account without returning a misleading
    // 404 to the admin panel.
    if subscription_id.is_none() {
        let account_id = account.id.clone();
        account.subscription = None;
        account.save(db).await?;
        let mut user = db.fetch_user(&account_id).await?;
        let badges = user.badges.unwrap_or_default() & !(UserBadges::TailsTalkSupporter as i32);
        user.update(
            db,
            PartialUser {
                badges: Some(badges),
                ..Default::default()
            },
            vec![],
        )
        .await?;
        return Ok(EmptyResponse);
    }
    let subscription_id = subscription_id.unwrap();

    if !stripe.enabled || stripe.secret_key.is_empty() {
        return Err(create_error!(FeatureDisabled { feature: "Stripe subscriptions".to_owned() }));
    }

    let client = reqwest::Client::new();

    if !immediate {
        let response = client
            .post(format!("https://api.stripe.com/v1/subscriptions/{subscription_id}"))
            .basic_auth(&stripe.secret_key, None::<&str>)
            .form(&[("cancel_at_period_end", "true")])
            .send()
            .await
            .map_err(|_| create_error!(InternalError))?;
        if !response.status().is_success() {
            log::error!("Stripe period-end cancellation failed with {}", response.status());
            return Err(create_error!(InternalError));
        }
        account
            .subscription
            .as_mut()
            .expect("subscription checked above")
            .cancel_at_period_end = true;
        account.save(db).await?;
        return Ok(EmptyResponse);
    }

    let response = client
        .delete(format!("https://api.stripe.com/v1/subscriptions/{subscription_id}"))
        .basic_auth(&stripe.secret_key, None::<&str>)
        .send()
        .await
        .map_err(|_| create_error!(InternalError))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.json::<Value>().await.unwrap_or_default();
        let code = body
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let message = body
            .get("error")
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        log::error!("Stripe subscription cancellation failed with {status}: {code}: {message}");

        // Cancellation is idempotent from the user's perspective. If the
        // subscription was already canceled in Stripe, reconcile local state.
        let lookup = client
            .get(format!("https://api.stripe.com/v1/subscriptions/{subscription_id}"))
            .basic_auth(&stripe.secret_key, None::<&str>)
            .send()
            .await
            .map_err(|_| create_error!(InternalError))?;
        if lookup.status().is_success() {
            let subscription = lookup.json::<Value>().await.map_err(|_| create_error!(InternalError))?;
            if subscription.get("status").and_then(Value::as_str) == Some("canceled") {
                let account_id = account.id.clone();
                account.subscription = None;
                account.save(db).await?;
                let mut user = db.fetch_user(&account_id).await?;
                let badges = user.badges.unwrap_or_default() & !(UserBadges::TailsTalkSupporter as i32);
                user.update(db, PartialUser { badges: Some(badges), ..Default::default() }, vec![]).await?;
                return Ok(EmptyResponse);
            }
        }
        return Err(create_error!(InternalError));
    }

    let account_id = account.id.clone();
    account.subscription = None;
    account.save(db).await?;
    let mut user = db.fetch_user(&account_id).await?;
    let badges = user.badges.unwrap_or_default() & !(UserBadges::TailsTalkSupporter as i32);
    user.update(
        db,
        PartialUser {
            badges: Some(badges),
            ..Default::default()
        },
        vec![],
    )
    .await?;
    Ok(EmptyResponse)
}
