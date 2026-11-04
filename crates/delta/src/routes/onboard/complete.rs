use once_cell::sync::Lazy;
use regex::Regex;
use revolt_database::{Database, PartialUser, User, Session};
use revolt_models::v0;
use revolt_result::{create_error, Result};

use rocket::{serde::json::Json, State};
use serde::{Deserialize, Serialize};
use validator::Validate;

/// Regex for valid usernames
///
/// Block zero width space
/// Block lookalike characters
pub static RE_USERNAME: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\p{L}|[\d_.-])+$").unwrap());

/// Legal names are stored privately and are never included in public user objects.
pub static RE_LEGAL_NAME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[^\u{200B}\n\r]+$").unwrap());

/// # New User Data
#[derive(Validate, Serialize, Deserialize, JsonSchema)]
pub struct DataOnboard {
    /// New username which will be used to identify the user on the platform
    #[validate(length(min = 2, max = 32), regex = "RE_USERNAME")]
    username: String,
    /// Private legal name used only for safety and legal requests.
    /// This is collected after signup by the required account modal.
    #[serde(default)]
    #[validate(length(min = 2, max = 100), regex = "RE_LEGAL_NAME")]
    legal_name: Option<String>,
}

/// # Complete Onboarding
///
/// This sets a new username, completes onboarding and allows a user to start using Revolt.
#[openapi(tag = "Onboarding")]
#[post("/complete", data = "<data>")]
pub async fn complete(
    db: &State<Database>,
    session: Session,
    user: Option<User>,
    data: Json<DataOnboard>,
) -> Result<Json<v0::User>> {
    if user.is_some() {
        return Err(create_error!(AlreadyOnboarded));
    }

    let data = data.into_inner();
    data.validate().map_err(|error| {
        create_error!(FailedValidation {
            error: error.to_string()
        })
    })?;

    if crate::util::username::is_disallowed_username(&data.username) {
        return Err(create_error!(FailedValidation {
            error: "You cannot use that username.".to_owned()
        }));
    }

    Ok(Json(
        User::create(
            db,
            data.username,
            session.user_id,
            Some(PartialUser {
                legal_name: data.legal_name,
                ..Default::default()
            }),
        )
            .await?
            .into_self(false)
            .await,
    ))
}
