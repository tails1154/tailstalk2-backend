//! Stripe subscription webhook.

use hmac::{Hmac, Mac};
use iso8601_timestamp::Timestamp;
use revolt_config::config;
use revolt_database::{
    mongodb::bson::{doc, DateTime, Document},
    Database, PartialUser, Subscription,
};
use revolt_models::v0::UserBadges;
use revolt_result::{create_error, Result};
use revolt_rocket_okapi::request::OpenApiFromRequest;
use rocket::{data::ToByteUnit, http::Status, request::FromRequest, Data, Request, State};
use serde::Deserialize;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use ulid::Ulid;

type HmacSha256 = Hmac<Sha256>;

#[derive(OpenApiFromRequest)]
pub struct StripeSignature(String);

#[rocket::async_trait]
impl<'r> FromRequest<'r> for StripeSignature {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> rocket::request::Outcome<Self, Self::Error> {
        match request.headers().get_one("Stripe-Signature") {
            Some(signature) => rocket::request::Outcome::Success(Self(signature.to_owned())),
            None => rocket::request::Outcome::Error((Status::BadRequest, ())),
        }
    }
}

#[derive(Debug, Deserialize)]
struct StripeEvent {
    id: String,
    #[serde(rename = "type")]
    event_type: String,
    data: StripeEventData,
}

#[derive(Debug, Deserialize)]
struct StripeEventData {
    object: serde_json::Value,
}

fn verify_signature(secret: &str, signature: &str, body: &[u8]) -> bool {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for item in signature.split(',') {
        let Some((key, value)) = item.split_once('=') else { continue };
        match key {
            "t" => timestamp = value.parse::<u64>().ok(),
            "v1" => signatures.push(value),
            _ => {}
        }
    }
    let Some(timestamp) = timestamp else { return false };
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else { return false };
    if now.as_secs().abs_diff(timestamp) > 300 { return false; }
    let payload = format!("{timestamp}.{}", String::from_utf8_lossy(body));
    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else { return false };
    mac.update(payload.as_bytes());
    signatures.iter().any(|signature| {
        let Ok(expected) = hex::decode(signature) else { return false };
        mac.clone().verify_slice(&expected).is_ok()
    })
}

fn active_subscription(event_type: &str, object: &serde_json::Value) -> Option<bool> {
    match event_type {
        "customer.subscription.created" | "customer.subscription.updated" => {
            let status = object.get("status")?.as_str()?;
            Some(matches!(status, "active" | "trialing"))
        }
        "customer.subscription.deleted" => Some(false),
        _ => None,
    }
}

async fn handle_boost_purchase(
    db: &State<Database>,
    event: &StripeEvent,
) -> Result<Status> {
    let object = &event.data.object;
    if object.get("payment_status").and_then(serde_json::Value::as_str) != Some("paid") {
        return Ok(Status::Ok);
    }
    let metadata = object.get("metadata");
    if metadata
        .and_then(|metadata| metadata.get("purchase_type"))
        .and_then(serde_json::Value::as_str)
        != Some("server_boost")
    {
        return Ok(Status::Ok);
    }
    let Some(account_id) = metadata
        .and_then(|metadata| metadata.get("tailstalk_account_id"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Status::Ok);
    };
    let Some(server_id) = metadata
        .and_then(|metadata| metadata.get("server_id"))
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(Status::Ok);
    };
    let Database::MongoDb(mongo) = db.inner() else {
        return Err(create_error!(InternalError));
    };
    if mongo
        .col::<Document>("server_boosts")
        .find_one(doc! { "stripe_event_id": &event.id })
        .await
        .map_err(|_| create_error!(InternalError))?
        .is_some()
    {
        return Ok(Status::Ok);
    }
    mongo
        .col::<Document>("server_boosts")
        .insert_one(doc! {
            "_id": Ulid::new().to_string(),
            "server_id": server_id,
            "user_id": account_id,
            "source": "purchased",
            "stripe_event_id": &event.id,
            "created_at": DateTime::now(),
        })
        .await
        .map_err(|_| create_error!(InternalError))?;
    Ok(Status::Ok)
}

/// Accept Stripe subscription lifecycle events and award the Supporter badge.
#[openapi(tag = "Webhooks")]
#[post("/stripe", data = "<data>")]
pub async fn stripe_webhook(
    signature: StripeSignature,
    db: &State<Database>,
    data: Data<'_>,
) -> Result<Status> {
    let settings = config().await;
    if !settings.api.stripe.enabled || settings.api.stripe.webhook_secret.is_empty() {
        return Err(create_error!(NotFound));
    }
    let body = data
        .open(1.megabytes())
        .into_bytes()
        .await
        .map_err(|_| create_error!(FailedValidation { error: "Invalid Stripe payload".to_owned() }))?;
    if !verify_signature(&settings.api.stripe.webhook_secret, &signature.0, &body.value) {
        return Err(create_error!(InvalidToken));
    }
    let event: StripeEvent = serde_json::from_slice(&body.value)
        .map_err(|_| create_error!(FailedValidation { error: "Invalid Stripe payload".to_owned() }))?;
    if event.event_type == "checkout.session.completed" {
        return handle_boost_purchase(db, &event).await;
    }
    let Some(active) = active_subscription(&event.event_type, &event.data.object) else {
        return Ok(Status::Ok);
    };
    let object = &event.data.object;
    let Some(account_id) = object
        .get("metadata")
        .and_then(|metadata| metadata.get("tailstalk_account_id"))
        .and_then(serde_json::Value::as_str)
    else {
        log::warn!("Stripe event {} has no tailstalk_account_id metadata", event.id);
        return Ok(Status::Ok);
    };
    let mut account = db.fetch_account(account_id).await?;
    if account.subscription.as_ref().is_some_and(|subscription| subscription.last_message_id == event.id) {
        return Ok(Status::Ok);
    }
    let subscription_id = object.get("id").and_then(serde_json::Value::as_str).map(str::to_owned);
    let customer_id = object.get("customer").and_then(serde_json::Value::as_str).map(str::to_owned);
    let cancel_at_period_end = object
        .get("cancel_at_period_end")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if !active {
        // Stripe cancellation events must never remove a gift entitlement.
        // Only clear the account when the event belongs to its current Stripe
        // subscription (older accounts may not have the provider id stored).
        let is_current_stripe_subscription = account.subscription.as_ref().is_some_and(|subscription| {
            subscription.provider == "stripe"
                && (subscription.provider_subscription_id.as_deref() == subscription_id.as_deref()
                    || subscription.provider_subscription_id.is_none())
        });
        if !is_current_stripe_subscription {
            return Ok(Status::Ok);
        }
        account.subscription = None;
        account.save(db).await?;
        let mut user = db.fetch_user(account_id).await?;
        let badges = user.badges.unwrap_or_default() & !(UserBadges::TailsTalkSupporter as i32);
        user.update(db, PartialUser { badges: Some(badges), ..Default::default() }, vec![]).await?;
        return Ok(Status::Ok);
    }
    if !settings.api.stripe.supporter_price_id.is_empty()
        && object.to_string().contains(&settings.api.stripe.supporter_price_id) == false
    {
        return Ok(Status::Ok);
    }
    account.subscription = Some(Subscription {
        provider: "stripe".to_owned(),
        tier: "TailsTalk 2 Premium".to_owned(),
        active_until: Timestamp::now_utc() + iso8601_timestamp::Duration::days(31),
        last_message_id: event.id,
        cancel_at_period_end,
        provider_customer_id: customer_id,
        provider_subscription_id: subscription_id,
    });
    account.save(db).await?;
    let mut user = db.fetch_user(account_id).await?;
    let badges = user.badges.unwrap_or_default() | UserBadges::TailsTalkSupporter as i32;
    user.update(db, PartialUser { badges: Some(badges), ..Default::default() }, vec![]).await?;
    Ok(Status::Ok)
}
