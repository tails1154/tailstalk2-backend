use once_cell::sync::Lazy;
use regex::Regex;
use revolt_database::{mongodb::bson::doc, Database, PartialUser, User};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use validator::Validate;

static RE_LEGAL_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[^\u{200B}\n\r]+$").unwrap());

#[derive(Debug, Serialize, JsonSchema)]
pub struct LegalNameStatus {
    pub set: bool,
}

#[derive(Debug, Deserialize, JsonSchema, Validate)]
pub struct SetLegalName {
    #[validate(length(min = 2, max = 100), regex = "RE_LEGAL_NAME")]
    pub legal_name: String,
}

fn mongo(db: &Database) -> Result<&revolt_database::MongoDb> {
    match db {
        Database::MongoDb(db) => Ok(db),
        _ => Err(create_error!(InternalError)),
    }
}

/// Check whether the signed-in user has supplied a private legal name.
#[openapi(tag = "User Information")]
#[get("/legal-name/status")]
pub async fn status(db: &State<Database>, user: User) -> Result<Json<LegalNameStatus>> {
    let document = mongo(db)?
        .col::<revolt_database::mongodb::bson::Document>("users")
        .find_one(doc! { "_id": &user.id })
        .await
        .map_err(|_| create_error!(InternalError))?
        .ok_or(create_error!(NotFound))?;

    Ok(Json(LegalNameStatus {
        set: document
            .get_str("legal_name")
            .map(|name| !name.trim().is_empty())
            .unwrap_or(false),
    }))
}

/// Store the signed-in user's private legal name.
#[openapi(tag = "User Information")]
#[patch("/legal-name", data = "<data>")]
pub async fn set(
    db: &State<Database>,
    mut user: User,
    data: Json<SetLegalName>,
) -> Result<Json<LegalNameStatus>> {
    let data = data.into_inner();
    data.validate().map_err(|error| {
        create_error!(FailedValidation {
            error: error.to_string()
        })
    })?;

    user.update(
        db,
        PartialUser {
            legal_name: Some(data.legal_name.trim().to_owned()),
            ..Default::default()
        },
        vec![],
    )
    .await?;

    Ok(Json(LegalNameStatus { set: true }))
}
