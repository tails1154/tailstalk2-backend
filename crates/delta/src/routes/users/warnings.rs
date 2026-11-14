use revolt_database::{mongodb::bson::doc, Database, User};
use revolt_result::Result;
use rocket::State;
use rocket::post;
use rocket::serde::json::Json;
use serde::Serialize;

#[derive(Serialize, JsonSchema, Debug)]
pub struct UserWarnings {
    pub warnings: Vec<WarningItem>,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct WarningItem {
    pub reason: String,
    pub created_at: String,
    /// "warning" for staff warnings, "crisis" for a crisis support notice
    pub kind: String,
}

/// # Get My Warnings
///
/// Get warnings for the currently logged-in user.
#[openapi(tag = "User Information")]
#[get("/@me/warnings")]
pub async fn get_warnings(
    user: User,
    db: &State<Database>,
) -> Result<Json<UserWarnings>> {
    let warnings = match db.inner() {
        Database::MongoDb(mongo) => {
            if let Ok(Some(doc)) = mongo
                .col::<revolt_database::mongodb::bson::Document>("users")
                .find_one(
                    doc! { "_id": &user.id },
                )
                .await
            {
                // Warnings created on or before the last acknowledgement are hidden
                let acknowledged_at = doc.get_str("warnings_acknowledged_at").ok();
                doc.get_array("admin_warnings")
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                let d = v.as_document()?;
                                let created_at = d.get_str("created_at").ok()?.to_string();
                                if acknowledged_at.is_some_and(|ack| created_at.as_str() <= ack) {
                                    return None;
                                }
                                Some(WarningItem {
                                    reason: d.get_str("reason").ok()?.to_string(),
                                    created_at,
                                    kind: d.get_str("kind").unwrap_or("warning").to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    Ok(Json(UserWarnings { warnings }))
}

/// # Acknowledge My Warnings
///
/// Mark the currently logged-in user's warnings as seen, so they are not shown again.
#[openapi(tag = "User Information")]
#[post("/@me/warnings/acknowledge")]
pub async fn acknowledge_warnings(
    user: User,
    db: &State<Database>,
) -> Result<rocket_empty::EmptyResponse> {
    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<revolt_database::mongodb::bson::Document>("users")
            .update_one(
                doc! { "_id": &user.id },
                doc! { "$set": { "warnings_acknowledged_at": chrono::Utc::now().to_rfc3339() } },
            )
            .await
            .map_err(|_| revolt_result::create_error!(InternalError))?;
    }

    Ok(rocket_empty::EmptyResponse)
}
