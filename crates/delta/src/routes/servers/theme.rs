use revolt_database::{util::{permissions::DatabasePermissionQuery, reference::Reference}, Database, User};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct ServerTheme {
    pub id: String,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ServerThemeUpdate {
    pub theme: Option<ServerTheme>,
}

fn valid_hex(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].chars().all(|character| character.is_ascii_hexdigit())
}

fn valid_theme(theme: &ServerTheme) -> bool {
    theme.id.len() <= 128
        && !theme.name.trim().is_empty()
        && theme.name.chars().count() <= 64
        && theme.description.as_deref().unwrap_or("").chars().count() <= 256
        && valid_hex(&theme.primary)
        && valid_hex(&theme.secondary)
        && valid_hex(&theme.background)
        && valid_hex(&theme.surface)
        && valid_hex(&theme.surface_high)
        && valid_hex(&theme.on_surface)
        && theme.gradient.starts_with("linear-gradient(")
        && theme.gradient.len() <= 512
}

async fn load(db: &Database, server_id: &str) -> Result<Option<ServerTheme>> {
    let Database::MongoDb(mongo) = db else { return Ok(None) };
    let document = mongo
        .col::<revolt_database::mongodb::bson::Document>("platform_config")
        .find_one(revolt_database::mongodb::bson::doc! { "_id": "server_themes" })
        .await
        .map_err(|_| create_error!(InternalError))?;
    let Some(document) = document else { return Ok(None) };
    let Some(entries) = document.get_array("entries").ok() else { return Ok(None) };
    Ok(entries.iter().find_map(|entry| {
        let entry = entry.as_document()?;
        if entry.get_str("server_id").ok()? != server_id { return None; }
        revolt_database::mongodb::bson::from_document(entry.clone()).ok()
    }))
}

async fn save(db: &Database, server_id: &str, theme: Option<&ServerTheme>) -> Result<()> {
    let Database::MongoDb(mongo) = db else { return Ok(()) };
    let collection = mongo.col::<revolt_database::mongodb::bson::Document>("platform_config");
    let current = collection
        .find_one(revolt_database::mongodb::bson::doc! { "_id": "server_themes" })
        .await
        .map_err(|_| create_error!(InternalError))?;
    let mut entries = current
        .and_then(|document| document.get_array("entries").ok().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.as_document().and_then(|doc| doc.get_str("server_id").ok()).map(|id| id != server_id).unwrap_or(true))
        .collect::<Vec<_>>();
    if let Some(theme) = theme {
        let mut document = revolt_database::mongodb::bson::to_document(theme).map_err(|_| create_error!(InternalError))?;
        document.insert("server_id", server_id);
        entries.push(revolt_database::mongodb::bson::Bson::Document(document));
    }
    collection
        .update_one(
            revolt_database::mongodb::bson::doc! { "_id": "server_themes" },
            revolt_database::mongodb::bson::doc! { "$set": { "entries": entries } },
        )
        .upsert(true)
        .await
        .map_err(|_| create_error!(InternalError))?;
    Ok(())
}

#[openapi(tag = "Server Information")]
#[get("/<target>/theme")]
pub async fn fetch(db: &State<Database>, user: User, target: Reference<'_>) -> Result<Json<Option<ServerTheme>>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ViewChannel)?;
    Ok(Json(load(db.inner(), &server.id).await?))
}

#[openapi(tag = "Server Information")]
#[patch("/<target>/theme", data = "<data>")]
pub async fn update(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<ServerThemeUpdate>,
) -> Result<Json<Option<ServerTheme>>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;
    let theme = data.into_inner().theme;
    if let Some(theme) = &theme {
        if !valid_theme(theme) {
            return Err(create_error!(FailedValidation { error: "Theme fields are invalid.".to_owned() }));
        }
    }
    save(db.inner(), &server.id, theme.as_ref()).await?;
    Ok(Json(theme))
}
