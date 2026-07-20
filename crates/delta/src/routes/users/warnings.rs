use revolt_database::{mongodb::bson::doc, Database, User};
use revolt_result::Result;
use rocket::State;
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
                doc.get_array("admin_warnings")
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                let d = v.as_document()?;
                                Some(WarningItem {
                                    reason: d.get_str("reason").ok()?.to_string(),
                                    created_at: d.get_str("created_at").ok()?.to_string(),
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
