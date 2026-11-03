use revolt_database::{mongodb::bson::{self, doc}, Database};
use revolt_result::{create_error, Result};
use rocket::serde::json::Json;
use rocket::State;
use rocket_empty::EmptyResponse;
use serde::{Deserialize, Serialize};

use super::auth::AdminUser;

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminSubscription {
    pub account_id: String,
    pub email: String,
    pub status: String,
    pub requested_at: String,
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct SubscriptionAction {
    pub status: String,
}

fn text(document: &bson::Document, key: &str) -> String {
    document.get_str(key).unwrap_or_default().to_owned()
}

#[openapi(tag = "Admin")]
#[get("/subscriptions")]
pub async fn admin_subscriptions(
    _admin: AdminUser,
    db: &State<Database>,
) -> Result<Json<Vec<AdminSubscription>>> {
    let Database::MongoDb(mongo) = db.inner() else {
        return Ok(Json(Vec::new()));
    };
    let mut cursor = mongo
        .col::<bson::Document>("accounts")
        .find(doc! { "cash_payment_request": { "$exists": true } })
        .await
        .map_err(|_| create_error!(InternalError))?;
    let mut requests = Vec::new();
    use futures::StreamExt;
    while let Some(account) = cursor.next().await {
        let account = account.map_err(|_| create_error!(InternalError))?;
        let Some(request) = account.get_document("cash_payment_request").ok() else {
            continue;
        };
        requests.push(AdminSubscription {
            account_id: text(&account, "_id"),
            email: text(&account, "email"),
            status: text(request, "status"),
            requested_at: request.get("requested_at").map(ToString::to_string).unwrap_or_default(),
        });
    }
    Ok(Json(requests))
}

#[openapi(tag = "Admin")]
#[post("/subscriptions/<account_id>", data = "<data>")]
pub async fn admin_update_subscription(
    _admin: AdminUser,
    db: &State<Database>,
    account_id: String,
    data: Json<SubscriptionAction>,
) -> Result<EmptyResponse> {
    if !matches!(data.status.as_str(), "approved" | "declined" | "completed") {
        return Err(create_error!(FailedValidation {
            error: "Unsupported subscription status".to_owned(),
        }));
    }
    let Database::MongoDb(mongo) = db.inner() else {
        return Err(create_error!(InternalError));
    };
    mongo
        .col::<bson::Document>("accounts")
        .update_one(
            doc! { "_id": &account_id },
            doc! { "$set": { "cash_payment_request.status": &data.status } },
        )
        .await
        .map_err(|_| create_error!(InternalError))?;
    crate::util::system_message::send_system_dm(
        db.inner(),
        &account_id,
        format!(
            "Your TailsTalk 2 Premium cash payment request was {}.",
            match data.status.as_str() {
                "approved" | "completed" => "accepted",
                _ => "declined",
            }
        ),
    )
    .await;
    Ok(EmptyResponse)
}
