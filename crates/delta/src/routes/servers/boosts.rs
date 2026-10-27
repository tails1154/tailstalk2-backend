//! Server boosts.
//!
//! Premium currently grants two boost slots. Purchased boosts can use the
//! same collection later by using `source: "purchased"`; purchase/provider
//! configuration deliberately stays outside this feature.

use iso8601_timestamp::Timestamp;
use revolt_config::config;
use revolt_database::{
    mongodb::bson::{doc, DateTime, Document},
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Account, Database, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission, PermissionQuery};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use serde::Serialize;
use ulid::Ulid;

const COLLECTION: &str = "server_boosts";
const PREMIUM_BOOSTS: i64 = 2;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServerBoost {
    pub id: String,
    pub user_id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ServerBoostSummary {
    pub server_id: String,
    pub boost_count: u64,
    pub level: u8,
    pub next_level: Option<u8>,
    pub user_boosts: Vec<ServerBoost>,
    pub available_premium_boosts: i64,
    pub is_premium: bool,
    pub progress_enabled: bool,
    pub can_manage: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BoostPurchaseResponse {
    pub url: String,
}

fn level_for(count: u64) -> (u8, Option<u8>) {
    match count {
        0..=1 => (0, Some(2)),
        2..=4 => (1, Some(5)),
        5..=9 => (2, Some(10)),
        _ => (3, None),
    }
}

fn active_premium(account: &Account) -> bool {
    account
        .subscription
        .as_ref()
        .is_some_and(|subscription| subscription.active_until > Timestamp::now_utc())
}

async fn server_for_user(
    db: &State<Database>,
    user: &User,
    target: Reference<'_>,
) -> Result<revolt_database::Server> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, user).server(&server);
    if !query.are_we_a_member().await {
        return Err(create_error!(NotFound));
    }
    Ok(server)
}

async fn mongo(db: &State<Database>) -> Result<&revolt_database::MongoDb> {
    match db.inner() {
        Database::MongoDb(mongo) => Ok(mongo),
        _ => Err(create_error!(InternalError)),
    }
}

async fn progress_enabled(db: &State<Database>, server_id: &str) -> Result<bool> {
    let mongo = mongo(db).await?;
    let document = mongo
        .col::<Document>("servers")
        .find_one(doc! { "_id": server_id })
        .await
        .map_err(|_| create_error!(InternalError))?;
    Ok(document
        .and_then(|document| document.get_bool("boost_progress_enabled").ok())
        .unwrap_or(false))
}

async fn records(
    db: &State<Database>,
    server_id: &str,
    user_id: Option<&str>,
) -> Result<Vec<ServerBoost>> {
    let mongo = mongo(db).await?;
    let mut filter = doc! { "server_id": server_id };
    if let Some(user_id) = user_id {
        filter.insert("user_id", user_id);
    }
    let documents = mongo
        .col::<Document>(COLLECTION)
        .find(filter)
        .await
        .map_err(|_| create_error!(InternalError))?;
    use futures::StreamExt;
    let documents: Vec<Document> = documents
        .filter_map(|document| async { document.ok() })
        .collect()
        .await;
    Ok(documents
        .into_iter()
        .filter_map(|document| {
            Some(ServerBoost {
                id: document.get_str("_id").ok()?.to_owned(),
                user_id: document.get_str("user_id").ok()?.to_owned(),
            })
        })
        .collect())
}

/// Fetch boost count, level, and the current user's assigned boosts.
#[openapi(tag = "Server Information")]
#[get("/<target>/boosts")]
pub async fn fetch(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerBoostSummary>> {
    let server = server_for_user(db, &user, target).await?;
    let mongo = mongo(db).await?;
    let boost_count = mongo
        .count_documents(COLLECTION, doc! { "server_id": &server.id })
        .await
        .map_err(|_| create_error!(InternalError))?;
    let user_boosts = records(db, &server.id, Some(&user.id)).await?;
    let account = db.fetch_account(&user.id).await?;
    let premium = active_premium(&account);
    let mut permissions = DatabasePermissionQuery::new(db, &user).server(&server);
    let can_manage = calculate_server_permissions(&mut permissions)
        .await
        .has_channel_permission(ChannelPermission::ManageServer);
    let assigned = mongo
        .count_documents(COLLECTION, doc! { "user_id": &user.id, "source": "subscription" })
        .await
        .map_err(|_| create_error!(InternalError))? as i64;
    let (level, next_level) = level_for(boost_count);

    Ok(Json(ServerBoostSummary {
        server_id: server.id.clone(),
        boost_count,
        level,
        next_level,
        user_boosts,
        available_premium_boosts: if premium {
            (PREMIUM_BOOSTS - assigned).max(0)
        } else {
            0
        },
        is_premium: premium,
        progress_enabled: progress_enabled(db, &server.id).await?,
        can_manage,
    }))
}

#[derive(Debug, serde::Deserialize, JsonSchema)]
pub struct BoostSettings {
    pub progress_enabled: bool,
}

/// Enable or disable the public server boost progress bar.
#[openapi(tag = "Server Information")]
#[patch("/<target>/boosts/settings", data = "<data>")]
pub async fn update_settings(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<BoostSettings>,
) -> Result<Json<ServerBoostSummary>> {
    let server = server_for_user(db, &user, target).await?;
    let mut permissions = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut permissions)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;
    let mongo = mongo(db).await?;
    mongo
        .col::<Document>("servers")
        .update_one(
            doc! { "_id": &server.id },
            doc! { "$set": { "boost_progress_enabled": data.progress_enabled } },
        )
        .await
        .map_err(|_| create_error!(InternalError))?;

    fetch(db, user, Reference::from_unchecked(&server.id)).await
}

/// Assign one of the current user's Premium boost slots to this server.
#[openapi(tag = "Server Information")]
#[post("/<target>/boosts")]
pub async fn add(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerBoostSummary>> {
    let server = server_for_user(db, &user, target).await?;
    let account = db.fetch_account(&user.id).await?;
    if !active_premium(&account) {
        return Err(create_error!(FeatureDisabled { feature: "Premium server boosts".to_owned() }));
    }
    let mongo = mongo(db).await?;
    let assigned = mongo
        .count_documents(COLLECTION, doc! { "user_id": &user.id, "source": "subscription" })
        .await
        .map_err(|_| create_error!(InternalError))?;
    if assigned >= PREMIUM_BOOSTS as u64 {
        return Err(create_error!(FailedValidation { error: "You have no Premium boosts available".to_owned() }));
    }

    let id = Ulid::new().to_string();
    mongo
        .col::<Document>(COLLECTION)
        .insert_one(doc! {
            "_id": &id,
            "server_id": &server.id,
            "user_id": &user.id,
            "source": "subscription",
            "created_at": DateTime::now(),
        })
        .await
        .map_err(|_| create_error!(InternalError))?;

    fetch(db, user, Reference::from_unchecked(&server.id)).await
}

/// Create a hosted Stripe Checkout session for one permanently purchased boost.
/// The Stripe price is configuration-driven and can be left empty to disable
/// purchases while the free Premium boost slots remain available.
#[openapi(tag = "Server Information")]
#[post("/<target>/boosts/purchase")]
pub async fn purchase(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<BoostPurchaseResponse>> {
    let server = server_for_user(db, &user, target).await?;
    let account = db.fetch_account(&user.id).await?;
    let settings = config().await;
    let stripe = &settings.api.stripe;
    if !stripe.enabled || stripe.secret_key.is_empty() || stripe.boost_price_id.is_empty() {
        return Err(create_error!(FeatureDisabled { feature: "Boost purchases".to_owned() }));
    }

    let response = reqwest::Client::new()
        .post("https://api.stripe.com/v1/checkout/sessions")
        .basic_auth(&stripe.secret_key, None::<&str>)
        .form(&[
            ("mode", "payment"),
            ("line_items[0][price]", stripe.boost_price_id.as_str()),
            ("line_items[0][quantity]", "1"),
            ("customer_email", account.email.as_str()),
            ("metadata[tailstalk_account_id]", user.id.as_str()),
            ("metadata[server_id]", server.id.as_str()),
            ("metadata[purchase_type]", "server_boost"),
            ("success_url", stripe.success_url.as_str()),
            ("cancel_url", stripe.cancel_url.as_str()),
        ])
        .send()
        .await
        .map_err(|_| create_error!(InternalError))?;
    if !response.status().is_success() {
        log::error!("Boost Checkout session creation failed with {}", response.status());
        return Err(create_error!(InternalError));
    }
    let session = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| create_error!(InternalError))?;
    let Some(url) = session.get("url").and_then(serde_json::Value::as_str) else {
        return Err(create_error!(InternalError));
    };
    Ok(Json(BoostPurchaseResponse { url: url.to_owned() }))
}

/// Remove one of the current user's assigned boosts from this server.
#[openapi(tag = "Server Information")]
#[delete("/<target>/boosts/<boost_id>")]
pub async fn remove(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    boost_id: &str,
) -> Result<Json<ServerBoostSummary>> {
    let server = server_for_user(db, &user, target).await?;
    let mongo = mongo(db).await?;
    let deleted = mongo
        .col::<Document>(COLLECTION)
        .delete_one(doc! {
            "_id": boost_id,
            "server_id": &server.id,
            "user_id": &user.id,
        })
        .await
        .map_err(|_| create_error!(InternalError))?;
    if deleted.deleted_count == 0 {
        return Err(create_error!(NotFound));
    }

    fetch(db, user, Reference::from_unchecked(&server.id)).await
}
