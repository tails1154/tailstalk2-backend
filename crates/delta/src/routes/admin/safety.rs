//! Review queue for AI safety flags. Only flags from school accounts are stored.
//! Message text is never stored here or returned, so reviewers open the message in its channel.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD, Engine};
use futures::StreamExt;
use revolt_rocket_okapi::{
    gen::OpenApiGenerator,
    request::{OpenApiFromRequest, RequestHeaderInput},
    revolt_okapi::openapi3::{MediaType, Parameter, ParameterValue},
};
use revolt_config::{config, ApiAisd};
use rocket::http::{Cookie, CookieJar, SameSite, Status};
use rocket::request::{FromRequest, Outcome, Request};
use rocket::response::Redirect;
use sha2::{Digest, Sha256};
use revolt_database::mongodb::bson::{self, doc};
use crate::util::safety::{in_scope, panel_access_allowed};
use revolt_database::{Database, EmailVerification, User};
use revolt_result::{create_error, Result};
use rocket::serde::json::Json;
use rocket::State;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};


#[derive(Serialize, JsonSchema, Debug)]
pub struct SafetyFlag {
    pub message_id: String,
    pub channel_id: String,
    pub author_id: String,
    pub kind: String,
    pub confidence: f64,
    pub created_at: String,
}

/// # List Safety Flags
///
/// Unreviewed flags from school accounts, newest first. Message text is not included.
#[openapi(tag = "Admin")]
#[get("/safety/flags")]
pub async fn list_flags(_panel: PanelUser, db: &State<Database>) -> Result<Json<Vec<SafetyFlag>>> {
    Ok(Json(school_flags(db.inner()).await?))
}

/// Open school-account flags, newest first. Shared by the HTTP list and the live websocket.
pub async fn school_flags(db: &Database) -> Result<Vec<SafetyFlag>> {
    let Database::MongoDb(mongo) = db else {
        return Ok(Vec::new());
    };

    let mut cursor = mongo
        .col::<bson::Document>("safety_flags")
        .find(doc! { "reviewed": false })
        .sort(doc! { "_id": -1 })
        .limit(100)
        .await
        .map_err(|_| create_error!(InternalError))?;

    let mut flags = Vec::new();
    let mut in_scope_cache: std::collections::HashMap<String, bool> = Default::default();
    while let Some(Ok(doc)) = cursor.next().await {
        let author_id = doc.get_str("author_id").unwrap_or_default().to_string();
        // Only school accounts are shown, checked again here so the panel never lists anyone else
        let allowed = match in_scope_cache.get(&author_id) {
            Some(v) => *v,
            None => {
                let v = school_account(db, &author_id).await.is_ok();
                in_scope_cache.insert(author_id.clone(), v);
                v
            }
        };
        if !allowed {
            continue;
        }
        flags.push(SafetyFlag {
            message_id: doc.get_str("message_id").unwrap_or_default().to_string(),
            channel_id: doc.get_str("channel_id").unwrap_or_default().to_string(),
            author_id,
            kind: doc.get_str("kind").unwrap_or_default().to_string(),
            confidence: doc.get_f64("confidence").unwrap_or_default(),
            created_at: doc.get_str("created_at").unwrap_or_default().to_string(),
        });
    }

    Ok(flags)
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct ContextMessage {
    pub id: String,
    pub username: String,
    pub content: String,
    /// True for the message that was flagged
    pub flagged: bool,
}

fn field(doc: &bson::Document, key: &str) -> String {
    doc.get_str(key).unwrap_or_default().to_string()
}

/// # Flag Context
///
/// The flagged message and the five messages before it in the same channel, oldest first.
/// Read live from the messages collection, and only for flags from school accounts.
#[openapi(tag = "Admin")]
#[get("/safety/flags/<message_id>/context")]
pub async fn flag_context(
    _panel: PanelUser,
    db: &State<Database>,
    message_id: String,
) -> Result<Json<Vec<ContextMessage>>> {
    let Database::MongoDb(mongo) = db.inner() else {
        return Err(create_error!(InternalError));
    };
    let flag = mongo
        .col::<bson::Document>("safety_flags")
        .find_one(doc! { "message_id": &message_id })
        .await
        .map_err(|_| create_error!(InternalError))?
        .ok_or_else(|| create_error!(NotFound))?;
    school_account(db, &field(&flag, "author_id")).await?;
    let channel_id = field(&flag, "channel_id");

    let messages = mongo.col::<bson::Document>("messages");
    let mut cursor = messages
        .find(doc! { "channel": &channel_id, "_id": { "$lte": &message_id } })
        .sort(doc! { "_id": -1 })
        .limit(6)
        .await
        .map_err(|_| create_error!(InternalError))?;
    let mut docs = Vec::new();
    while let Some(Ok(d)) = cursor.next().await {
        docs.push(d);
    }
    docs.reverse();

    let author_ids: Vec<String> = docs.iter().map(|d| field(d, "author")).collect();
    let mut names = std::collections::HashMap::new();
    let mut users = mongo
        .col::<bson::Document>("users")
        .find(doc! { "_id": { "$in": &author_ids } })
        .await
        .map_err(|_| create_error!(InternalError))?;
    while let Some(Ok(user)) = users.next().await {
        names.insert(field(&user, "_id"), field(&user, "username"));
    }

    Ok(Json(
        docs.iter()
            .map(|d| {
                let author = field(d, "author");
                ContextMessage {
                    id: field(d, "_id"),
                    username: names.get(&author).cloned().unwrap_or(author),
                    content: field(d, "content"),
                    flagged: field(d, "_id") == message_id,
                }
            })
            .collect(),
    ))
}

/// Fetches an account only when it is verified and on the school domain. Everything else is NotFound.
async fn school_account(db: &Database, user_id: &str) -> Result<revolt_database::Account> {
    let account = db.fetch_account(user_id).await.map_err(|_| create_error!(NotFound))?;
    if !matches!(account.verification, EmailVerification::Verified) || !in_scope(&account.email) {
        return Err(create_error!(NotFound));
    }
    Ok(account)
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct SchoolAccount {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub email: String,
    pub suspended_until: Option<String>,
    pub banned: bool,
    pub warnings: u32,
}

/// # School Account
///
/// Account details for a school account. Other accounts return not found.
#[openapi(tag = "Admin")]
#[get("/safety/users/<id>")]
pub async fn school_account_info(
    _panel: PanelUser,
    db: &State<Database>,
    id: String,
) -> Result<Json<SchoolAccount>> {
    let account = school_account(db, &id).await?;
    let user = db.fetch_user(&id).await?;
    let Database::MongoDb(mongo) = db.inner() else {
        return Err(create_error!(InternalError));
    };
    let raw = mongo
        .col::<bson::Document>("users")
        .find_one(doc! { "_id": &id })
        .await
        .map_err(|_| create_error!(InternalError))?
        .unwrap_or_default();
    Ok(Json(SchoolAccount {
        id,
        username: user.username,
        display_name: user.display_name,
        email: account.email,
        suspended_until: user.suspended_until.map(|t| t.to_string()),
        banned: user.flags.unwrap_or(0) & 4 != 0,
        warnings: raw.get_array("admin_warnings").map(|w| w.len() as u32).unwrap_or(0),
    }))
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct PanelSuspendData {
    /// Hours to suspend for, from 1 to 720
    pub hours: i64,
    /// Short reason, stored on the account. Sent to the user only at login. No email is sent.
    pub reason: Option<String>,
}

/// # Suspend School Account
///
/// Staff action from the panel. Limited to school accounts and recorded in the audit log.
#[openapi(tag = "Admin")]
#[post("/safety/users/<id>/suspend", data = "<data>")]
pub async fn suspend_school_account(
    panel: PanelUser,
    db: &State<Database>,
    id: String,
    data: Json<PanelSuspendData>,
) -> Result<Json<serde_json::Value>> {
    if !(1..=720).contains(&data.hours) {
        return Err(create_error!(InvalidOperation));
    }
    school_account(db, &id).await?;
    let until = iso8601_timestamp::Timestamp::now_utc()
        + iso8601_timestamp::Duration::hours(data.hours);
    let mut user = db.fetch_user(&id).await?;
    // Reason is None so the database does not send the suspension email
    user.suspend(db, Some(until), None).await?;
    if let Database::MongoDb(mongo) = db.inner() {
        if let Some(reason) = data.reason.as_deref() {
            mongo
                .col::<bson::Document>("users")
                .update_one(doc! { "_id": &id }, doc! { "$set": { "admin_suspend_reason": reason } })
                .await
                .map_err(|_| create_error!(InternalError))?;
        }
    }
    // Taking action resolves the account's open flags
    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<bson::Document>("safety_flags")
            .update_many(
                doc! { "author_id": &id, "reviewed": false },
                doc! { "$set": { "reviewed": true, "resolved_by": &panel.email } },
            )
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    audit(db, &panel.email, "suspend", &id, &format!("{}h", data.hours)).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Unsuspend School Account
///
/// Staff action from the panel. Limited to school accounts and recorded in the audit log.
#[openapi(tag = "Admin")]
#[post("/safety/users/<id>/unsuspend")]
pub async fn unsuspend_school_account(
    panel: PanelUser,
    db: &State<Database>,
    id: String,
) -> Result<Json<serde_json::Value>> {
    school_account(db, &id).await?;
    let mut user = db.fetch_user(&id).await?;
    if user.flags.unwrap_or(0) & 4 != 0 {
        return Err(create_error!(Banned));
    }
    user.unsuspend(db).await?;
    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<bson::Document>("users")
            .update_one(doc! { "_id": &id }, doc! { "$unset": { "admin_suspend_reason": "" as &str } })
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    audit(db, &panel.email, "unsuspend", &id, "").await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct PanelBanData {
    /// Short reason, stored on the account. No email is sent.
    pub reason: Option<String>,
}

/// # Ban School Account
///
/// Staff action from the panel. Ends all sessions, blocks login, and is recorded in the audit log.
#[openapi(tag = "Admin")]
#[post("/safety/users/<id>/ban", data = "<data>")]
pub async fn ban_school_account(
    panel: PanelUser,
    db: &State<Database>,
    id: String,
    data: Option<Json<PanelBanData>>,
) -> Result<Json<serde_json::Value>> {
    school_account(db, &id).await?;
    let reason = data.and_then(|d| d.reason.clone());

    // Disabling the account through the model removes its active sessions
    let mut account = db.fetch_account(&id).await?;
    account.disable(db).await?;

    if let Database::MongoDb(mongo) = db.inner() {
        let mut set = doc! { "flags": 4i32 };
        if let Some(ref r) = reason {
            set.insert("admin_ban_reason", r.as_str());
        }
        mongo
            .col::<bson::Document>("users")
            .update_one(doc! { "_id": &id }, doc! { "$set": set })
            .await
            .map_err(|_| create_error!(InternalError))?;
        // Taking action resolves the account's open flags
        mongo
            .col::<bson::Document>("safety_flags")
            .update_many(
                doc! { "author_id": &id, "reviewed": false },
                doc! { "$set": { "reviewed": true, "resolved_by": &panel.email } },
            )
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    audit(db, &panel.email, "ban", &id, reason.as_deref().unwrap_or("")).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Records who did what in the panel, for later review
async fn audit(db: &Database, actor: &str, action: &str, target: &str, detail: &str) {
    let Database::MongoDb(mongo) = db else {
        return;
    };
    let result = mongo
        .col::<bson::Document>("safety_panel_audit")
        .insert_one(doc! {
            "actor": actor,
            "action": action,
            "target": target,
            "detail": detail,
            "at": iso8601_timestamp::Timestamp::now_utc().to_string(),
        })
        .await;
    if let Err(err) = result {
        log::error!("Failed to write panel audit entry: {err:?}");
    }
}

/// # Mark Safety Flag Reviewed
///
/// Marks every flag for a message as reviewed.
#[openapi(tag = "Admin")]
#[post("/safety/flags/<message_id>/review")]
pub async fn review_flag(
    _panel: PanelUser,
    db: &State<Database>,
    message_id: String,
) -> Result<Json<serde_json::Value>> {
    let Database::MongoDb(mongo) = db.inner() else {
        return Err(create_error!(InternalError));
    };

    mongo
        .col::<bson::Document>("safety_flags")
        .update_many(
            doc! { "message_id": &message_id },
            doc! { "$set": { "reviewed": true } },
        )
        .await
        .map_err(|_| create_error!(InternalError))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct PanelAccess {
    /// Whether this account may open the AISD review panel
    pub allowed: bool,
    /// Full URL of the panel
    pub url: String,
}

/// # Panel Access
///
/// Whether the signed-in account may open the AISD review panel. Access is decided here, not in the client.
#[openapi(tag = "Admin")]
#[get("/safety/panel-access")]
pub async fn panel_access(user: User, db: &State<Database>) -> Result<Json<PanelAccess>> {
    let settings = config().await.api.aisd;
    let account = db.fetch_account(&user.id).await?;
    let allowed = matches!(account.verification, EmailVerification::Verified)
        && panel_access_allowed(&account.email, &settings);
    Ok(Json(PanelAccess { allowed, url: settings.panel_url.clone() }))
}

const SESSION_COOKIE: &str = "aisd_panel";
const STATE_COOKIE: &str = "aisd_oauth_state";
const SESSION_SECONDS: i64 = 8 * 60 * 60;

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Sessions are stored as hashes, so a leaked database does not expose working tokens
fn hash_token(token: &str) -> String {
    STANDARD.encode(Sha256::digest(token.as_bytes()))
}

fn session_cookie(value: String, max_age_secs: i64) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .max_age(rocket::time::Duration::seconds(max_age_secs))
        .build()
}

/// A signed-in panel session from the Google sign-in cookie. The email must still be on the allowlist.
pub struct PanelUser {
    pub email: String,
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for PanelUser {
    type Error = ();

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let Some(cookie) = request.cookies().get(SESSION_COOKIE) else {
            return Outcome::Error((Status::Unauthorized, ()));
        };
        let Some(db) = request.rocket().state::<Database>() else {
            return Outcome::Error((Status::InternalServerError, ()));
        };
        let Database::MongoDb(mongo) = db else {
            return Outcome::Error((Status::ServiceUnavailable, ()));
        };

        let found = mongo
            .col::<bson::Document>("safety_panel_sessions")
            .find_one(doc! { "token_hash": hash_token(cookie.value()) })
            .await;
        let Ok(Some(session)) = found else {
            return Outcome::Error((Status::Unauthorized, ()));
        };
        if session.get_i64("expires_at").unwrap_or(0) < now_secs() {
            return Outcome::Error((Status::Unauthorized, ()));
        }

        let email = session.get_str("email").unwrap_or_default().to_string();
        let aisd = config().await.api.aisd;
        if !panel_access_allowed(&email, &aisd) {
            return Outcome::Error((Status::Forbidden, ()));
        }
        Outcome::Success(PanelUser { email })
    }
}

/// Starts Google sign-in. The state value is kept in a short-lived cookie and checked on return.
#[get("/safety/oauth/google/start")]
pub async fn google_start(cookies: &CookieJar<'_>) -> Redirect {
    let aisd = config().await.api.aisd;
    let state = nanoid::nanoid!(32);
    cookies.add(
        Cookie::build((STATE_COOKIE, state.clone()))
            .path("/")
            .http_only(true)
            .secure(true)
            .same_site(SameSite::Lax)
            .max_age(rocket::time::Duration::minutes(10))
            .build(),
    );

    let url = reqwest::Url::parse_with_params(
        "https://accounts.google.com/o/oauth2/v2/auth",
        &[
            ("client_id", aisd.google_client_id.as_str()),
            ("redirect_uri", aisd.panel_redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", "openid email profile"),
            ("state", state.as_str()),
            ("prompt", "select_account"),
        ],
    )
    .expect("valid Google auth URL");
    Redirect::to(url.to_string())
}

async fn email_from_google(aisd: &ApiAisd, code: &str) -> Result<String, String> {
    let token: serde_json::Value = reqwest::Client::new()
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("code", code),
            ("client_id", aisd.google_client_id.as_str()),
            ("client_secret", aisd.google_client_secret.as_str()),
            ("redirect_uri", aisd.panel_redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let access_token = token["access_token"].as_str().ok_or("no access token")?;
    let info: serde_json::Value = reqwest::Client::new()
        .get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    if info["email_verified"].as_bool() != Some(true) {
        return Err("email not verified".into());
    }
    info["email"]
        .as_str()
        .map(|e| e.to_lowercase())
        .ok_or_else(|| "no email in Google response".to_string())
}

/// Google returns here. Only allowlisted, verified addresses get a panel session.
#[get("/safety/oauth/google/callback?<code>&<state>&<error>")]
pub async fn google_callback(
    cookies: &CookieJar<'_>,
    db: &State<Database>,
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
) -> Redirect {
    let aisd = config().await.api.aisd;
    let panel = aisd.panel_url.clone();

    let expected = cookies.get(STATE_COOKIE).map(|c| c.value().to_string());
    cookies.remove(Cookie::build(STATE_COOKIE).path("/").build());

    let (Some(code), Some(state)) = (code, state) else {
        return Redirect::to(format!("{panel}?error=cancelled"));
    };
    if error.is_some() || expected.as_deref() != Some(state.as_str()) {
        return Redirect::to(format!("{panel}?error=signin_failed"));
    }

    let email = match email_from_google(&aisd, &code).await {
        Ok(email) => email,
        Err(err) => {
            log::error!("Panel sign-in failed: {err}");
            return Redirect::to(format!("{panel}?error=signin_failed"));
        }
    };
    if !panel_access_allowed(&email, &aisd) {
        return Redirect::to(format!("{panel}?error=not_allowed"));
    }

    let Database::MongoDb(mongo) = db.inner() else {
        return Redirect::to(format!("{panel}?error=unavailable"));
    };
    let token = nanoid::nanoid!(48);
    let stored = mongo
        .col::<bson::Document>("safety_panel_sessions")
        .insert_one(doc! {
            "token_hash": hash_token(&token),
            "email": &email,
            "expires_at": now_secs() + SESSION_SECONDS,
        })
        .await;
    if stored.is_err() {
        return Redirect::to(format!("{panel}?error=unavailable"));
    }

    cookies.add(session_cookie(token, SESSION_SECONDS));
    Redirect::to(panel)
}

/// Returns 200 for a signed-in, allowed panel user. 401 means signed out, 403 means not allowed.
#[get("/safety/oauth/me")]
pub async fn whoami(panel: PanelUser) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "email": panel.email, "allowed": true }))
}

/// Ends the panel session and clears its cookie
#[post("/safety/oauth/logout")]
pub async fn logout(cookies: &CookieJar<'_>, db: &State<Database>) -> Status {
    if let (Some(cookie), Database::MongoDb(mongo)) = (cookies.get(SESSION_COOKIE), db.inner()) {
        let _ = mongo
            .col::<bson::Document>("safety_panel_sessions")
            .delete_one(doc! { "token_hash": hash_token(cookie.value()) })
            .await;
    }
    cookies.remove(Cookie::build(SESSION_COOKIE).path("/").build());
    Status::NoContent
}

/// Live queue for the panel. Sends the flag list whenever it changes, checked once a second on the server.
/// The socket needs a signed-in panel session, so it is refused for anyone else.
#[get("/safety/ws")]
pub fn safety_ws(
    _panel: PanelUser,
    db: &State<Database>,
    ws: rocket_ws::WebSocket,
) -> rocket_ws::Channel<'static> {
    let db = db.inner().clone();
    ws.channel(move |mut stream| {
        Box::pin(async move {
            use futures::SinkExt;
            use rocket_ws::Message;

            let mut last = String::new();
            loop {
                if let Ok(flags) = school_flags(&db).await {
                    let json = serde_json::to_string(&flags).unwrap_or_else(|_| "[]".into());
                    if json != last {
                        if stream.send(Message::Text(json.clone())).await.is_err() {
                            break;
                        }
                        last = json;
                    }
                }
                // Wait up to a second for a client message or close; a timeout just means check again
                match tokio::time::timeout(std::time::Duration::from_secs(1), stream.next()).await {
                    Ok(None) | Ok(Some(Err(_))) => break,
                    _ => {}
                }
            }
            Ok::<(), rocket_ws::result::Error>(())
        })
    })
}

/// Routes that use cookies and redirects instead of the OpenAPI-annotated JSON API
pub fn oauth_routes() -> Vec<rocket::Route> {
    rocket::routes![google_start, google_callback, whoami, logout, safety_ws]
}

impl<'r> OpenApiFromRequest<'r> for PanelUser {
    fn from_request_input(
        _gen: &mut OpenApiGenerator,
        _name: String,
        _required: bool,
    ) -> revolt_rocket_okapi::Result<RequestHeaderInput> {
        // Authenticated with the aisd_panel cookie set by Google sign-in
        Ok(RequestHeaderInput::None)
    }
}
