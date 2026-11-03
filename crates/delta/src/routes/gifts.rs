//! Redeem administrator-created TailsTalk 2 Premium gifts.

use iso8601_timestamp::{Duration, Timestamp};
use revolt_database::{
    mongodb::bson::{doc, DateTime},
    Account, Database, PartialUser, Subscription,
};
use revolt_models::v0::UserBadges;
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use serde::Serialize;

#[derive(Serialize, JsonSchema)]
pub struct GiftRedemption {
    pub active_until: Timestamp,
}

#[openapi(tag = "Gifts")]
#[post("/<gift_id>")]
pub async fn redeem_gift(
    db: &State<Database>,
    account: Account,
    gift_id: String,
) -> Result<Json<GiftRedemption>> {
    let Database::MongoDb(mongo) = db.inner() else {
        return Err(create_error!(InternalError));
    };
    let gifts = mongo.col::<revolt_database::mongodb::bson::Document>("premium_gifts");
    let now = Timestamp::now_utc();
    if account
        .subscription
        .as_ref()
        .is_some_and(|subscription| subscription.active_until > now)
    {
        return Err(create_error!(FailedValidation {
            error: "This account already has active Premium".to_owned()
        }));
    }
    let gift = gifts
        .find_one(doc! { "_id": &gift_id, "revoked": false, "redeemed_by": { "$exists": false } })
        .await
        .map_err(|_| create_error!(InternalError))?
        .ok_or_else(|| create_error!(NotFound))?;
    let duration_days = gift
        .get_i64("duration_days")
        .map_err(|_| create_error!(InternalError))?;
    let active_until = now + Duration::days(duration_days);
    let updated = gifts
        .update_one(
            doc! { "_id": &gift_id, "revoked": false, "redeemed_by": { "$exists": false } },
            doc! { "$set": { "redeemed_by": &account.id, "redeemed_at": DateTime::now() } },
        )
        .await
        .map_err(|_| create_error!(InternalError))?;
    if updated.modified_count != 1 {
        return Err(create_error!(NotFound));
    }

    let mut account = account;
    account.subscription = Some(Subscription {
        provider: "gift".to_owned(),
        tier: "TailsTalk 2 Premium".to_owned(),
        active_until,
        last_message_id: gift_id,
        cancel_at_period_end: false,
        provider_customer_id: None,
        provider_subscription_id: None,
    });
    account.save(db).await?;
    let mut user = db.fetch_user(&account.id).await?;
    let badges = user.badges.unwrap_or_default() | UserBadges::TailsTalkSupporter as i32;
    user.update(
        db,
        PartialUser {
            badges: Some(badges),
            ..Default::default()
        },
        vec![],
    )
    .await?;
    Ok(Json(GiftRedemption { active_until }))
}
