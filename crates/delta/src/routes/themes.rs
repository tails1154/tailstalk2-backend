use revolt_database::{mongodb::bson::{self, doc, Document}, Database, User};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

const CONFIG_ID: &str = "theme_store";

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ThemeEntry {
    pub id: String,
    pub owner_id: String,
    pub name: String,
    pub description: String,
    pub primary: String,
    pub secondary: String,
    pub background: String,
    pub surface: String,
    pub surface_high: String,
    pub on_surface: String,
    pub gradient: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ThemeSubmission {
    pub name: String,
    pub description: Option<String>,
    pub primary: String,
    pub secondary: String,
    pub background: String,
    pub surface: String,
    pub surface_high: String,
    pub on_surface: String,
    pub gradient: String,
}

fn valid_hex(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].chars().all(|character| character.is_ascii_hexdigit())
}

fn valid_submission(data: &ThemeSubmission) -> bool {
    !data.name.trim().is_empty()
        && data.name.chars().count() <= 64
        && data.description.as_deref().unwrap_or("").chars().count() <= 256
        && valid_hex(&data.primary)
        && valid_hex(&data.secondary)
        && valid_hex(&data.background)
        && valid_hex(&data.surface)
        && valid_hex(&data.surface_high)
        && valid_hex(&data.on_surface)
        && data.gradient.starts_with("linear-gradient(")
        && data.gradient.len() <= 512
}

fn from_doc(document: &Document) -> Option<ThemeEntry> {
    Some(ThemeEntry {
        id: document.get_str("id").ok()?.to_owned(),
        owner_id: document.get_str("owner_id").ok()?.to_owned(),
        name: document.get_str("name").ok()?.to_owned(),
        description: document.get_str("description").unwrap_or_default().to_owned(),
        primary: document.get_str("primary").ok()?.to_owned(),
        secondary: document.get_str("secondary").ok()?.to_owned(),
        background: document.get_str("background").ok()?.to_owned(),
        surface: document.get_str("surface").ok()?.to_owned(),
        surface_high: document.get_str("surface_high").ok()?.to_owned(),
        on_surface: document.get_str("on_surface").ok()?.to_owned(),
        gradient: document.get_str("gradient").ok()?.to_owned(),
    })
}

async fn load(db: &Database) -> Result<Vec<Document>> {
    match db {
        Database::MongoDb(mongo) => Ok(mongo
            .col::<Document>("platform_config")
            .find_one(doc! { "_id": CONFIG_ID })
            .await
            .map_err(|_| create_error!(InternalError))?
            .and_then(|document| document.get_array("entries").ok().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| value.as_document().cloned())
            .collect()),
        _ => Ok(Vec::new()),
    }
}

async fn save(db: &Database, entries: Vec<Document>) -> Result<()> {
    if let Database::MongoDb(mongo) = db {
        let entries = bson::to_bson(&entries).map_err(|_| create_error!(InternalError))?;
        mongo
            .col::<Document>("platform_config")
            .update_one(
                doc! { "_id": CONFIG_ID },
                doc! { "$set": { "entries": entries } },
            )
            .upsert(true)
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    Ok(())
}

/// List themes published to the community theme store.
#[openapi(tag = "Core")]
#[get("/themes")]
pub async fn list(db: &State<Database>) -> Result<Json<Vec<ThemeEntry>>> {
    Ok(Json(load(db.inner())
        .await?
        .iter()
        .filter_map(from_doc)
        .collect()))
}

/// Fetch one published theme.
#[openapi(tag = "Core")]
#[get("/themes/<id>")]
pub async fn fetch(db: &State<Database>, id: String) -> Result<Json<ThemeEntry>> {
    load(db.inner())
        .await?
        .iter()
        .filter_map(from_doc)
        .find(|theme| theme.id == id)
        .map(Json)
        .ok_or(create_error!(NotFound))
}

/// Publish a theme to the community theme store.
#[openapi(tag = "User Information")]
#[post("/themes", data = "<data>")]
pub async fn create(
    db: &State<Database>,
    user: User,
    data: Json<ThemeSubmission>,
) -> Result<Json<ThemeEntry>> {
    let data = data.into_inner();
    if !valid_submission(&data) {
        return Err(create_error!(FailedValidation {
            error: "Theme fields are invalid.".to_owned()
        }));
    }

    let entry = ThemeEntry {
        id: Ulid::new().to_string(),
        owner_id: user.id,
        name: data.name.trim().to_owned(),
        description: data.description.unwrap_or_default().trim().to_owned(),
        primary: data.primary,
        secondary: data.secondary,
        background: data.background,
        surface: data.surface,
        surface_high: data.surface_high,
        on_surface: data.on_surface,
        gradient: data.gradient,
    };
    let mut entries = load(db.inner()).await?;
    entries.push(bson::to_document(&entry).map_err(|_| create_error!(InternalError))?);
    save(db.inner(), entries).await?;
    Ok(Json(entry))
}

/// Remove a theme published by the authenticated user.
#[openapi(tag = "User Information")]
#[delete("/themes/<id>")]
pub async fn delete(db: &State<Database>, user: User, id: String) -> Result<()> {
    let mut entries = load(db.inner()).await?;
    let original_len = entries.len();
    entries.retain(|document| {
        !(document.get_str("id") == Ok(id.as_str())
            && document.get_str("owner_id") == Ok(user.id.as_str()))
    });
    if entries.len() == original_len {
        return Err(create_error!(NotFound));
    }
    save(db.inner(), entries).await
}
