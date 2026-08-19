use revolt_database::{mongodb::bson::{self, doc}, Database};
use revolt_result::Result;
use rocket::State;
use rocket::serde::json::Json;
use serde::Serialize;

#[derive(Serialize, JsonSchema, Debug)]
pub struct WhatsNewEntry {
    pub id: String,
    pub title: String,
    pub body: String,
    pub date: String,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct WhatsNewData {
    pub entries: Vec<WhatsNewEntry>,
}

/// # Get What's New
///
/// Get the current What's New content. Public endpoint.
#[openapi(tag = "Core")]
#[get("/whatsnew")]
pub async fn get_whatsnew(db: &State<Database>) -> Result<Json<WhatsNewData>> {
    let entries = match db.inner() {
        Database::MongoDb(mongo) => {
            if let Ok(Some(doc)) = mongo
                .col::<bson::Document>("platform_config")
                .find_one(doc! { "_id": "whatsnew" })
                .await
            {
                doc.get_array("entries")
                    .map(|arr| {
                        arr.iter()
                            .enumerate()
                            .filter_map(|(index, v)| {
                                let d = v.as_document()?;
                                Some(WhatsNewEntry {
                                    id: d
                                        .get_str("id")
                                        .map(str::to_string)
                                        .unwrap_or_else(|_| format!("legacy-{index}")),
                                    title: d.get_str("title").unwrap_or("").to_string(),
                                    body: d.get_str("body").unwrap_or("").to_string(),
                                    date: d.get_str("date").unwrap_or("").to_string(),
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

    Ok(Json(WhatsNewData { entries }))
}
