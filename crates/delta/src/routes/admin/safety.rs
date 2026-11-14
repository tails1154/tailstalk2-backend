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
    /// Resolved flags stay listed so staff can reopen them
    pub reviewed: bool,
    /// Why the model flagged it, one sentence
    pub reason: String,
    /// The model's reasoning trace, when it gave one
    pub reasoning: String,
    /// Staff actions taken on this flag, oldest first
    pub actions: Vec<FlagAction>,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct FlagAction {
    pub action: String,
    pub actor: String,
    pub at: String,
    pub detail: String,
}

/// Which flag a panel action was taken from, so its history shows on that card
#[derive(Deserialize, JsonSchema, Debug, Default)]
pub struct PanelFlagRef {
    pub message_id: Option<String>,
}

/// # List Safety Flags
///
/// Flags from school accounts, newest first, open and resolved. Message text is not included.
#[openapi(tag = "Admin")]
#[get("/safety/flags")]
pub async fn list_flags(_panel: PanelUser, db: &State<Database>) -> Result<Json<Vec<SafetyFlag>>> {
    Ok(Json(school_flags(db.inner()).await?))
}

/// School-account flags, newest first, open and resolved. Shared by the HTTP list and the live websocket.
pub async fn school_flags(db: &Database) -> Result<Vec<SafetyFlag>> {
    let Database::MongoDb(mongo) = db else {
        return Ok(Vec::new());
    };

    let mut cursor = mongo
        .col::<bson::Document>("safety_flags")
        .find(doc! {})
        .sort(doc! { "_id": -1 })
        .limit(200)
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
            reviewed: doc.get_bool("reviewed").unwrap_or(false),
            reason: field(&doc, "reason"),
            reasoning: field(&doc, "reasoning"),
            actions: doc
                .get_array("actions")
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|a| a.as_document())
                        .map(|a| FlagAction {
                            action: field(a, "action"),
                            actor: field(a, "actor"),
                            at: field(a, "at"),
                            detail: field(a, "detail"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
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
/// The flagged message, the five messages before and after it in its channel, and replies to it. Oldest first.
/// Read live from the messages collection, and only for flags from school accounts.
#[openapi(tag = "Admin")]
#[get("/safety/flags/<message_id>/context")]
pub async fn flag_context(
    _panel: PanelUser,
    db: &State<Database>,
    message_id: String,
) -> Result<Json<Vec<ContextMessage>>> {
    Ok(Json(flag_context_data(db.inner(), &message_id).await?))
}

/// Shared by the HTTP route and the live websocket
pub async fn flag_context_data(db: &Database, message_id: &str) -> Result<Vec<ContextMessage>> {
    let Database::MongoDb(mongo) = db else {
        return Err(create_error!(InternalError));
    };
    let flag = mongo
        .col::<bson::Document>("safety_flags")
        .find_one(doc! { "message_id": message_id })
        .await
        .map_err(|_| create_error!(InternalError))?
        .ok_or_else(|| create_error!(NotFound))?;
    school_account(db, &field(&flag, "author_id")).await?;
    let channel_id = field(&flag, "channel_id");

    let messages = mongo.col::<bson::Document>("messages");
    let mut docs: Vec<bson::Document> = Vec::new();
    // Five before, the flagged message and five after, and any replies to the flagged message
    let queries: [(bson::Document, i32, i64); 3] = [
        (doc! { "channel": &channel_id, "_id": { "$lt": message_id } }, -1, 5),
        (doc! { "channel": &channel_id, "_id": { "$gte": message_id } }, 1, 6),
        (doc! { "replies": message_id }, 1, 20),
    ];
    for (filter, sort, limit) in queries {
        let mut cursor = messages
            .find(filter)
            .sort(doc! { "_id": sort })
            .limit(limit)
            .await
            .map_err(|_| create_error!(InternalError))?;
        while let Some(Ok(d)) = cursor.next().await {
            docs.push(d);
        }
    }
    docs.sort_by(|a, b| field(a, "_id").cmp(&field(b, "_id")));
    docs.dedup_by(|a, b| field(a, "_id") == field(b, "_id"));

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

    Ok(docs
        .iter()
        .map(|d| {
            let author = field(d, "author");
            ContextMessage {
                id: field(d, "_id"),
                username: names.get(&author).cloned().unwrap_or(author),
                content: field(d, "content"),
                flagged: field(d, "_id") == message_id,
            }
        })
        .collect())
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
    /// The flag the action was taken from, if any
    pub message_id: Option<String>,
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
    let reason = data.reason.as_deref().map(str::trim).unwrap_or_default().to_string();
    if reason.is_empty() || reason.len() > 500 {
        return Err(create_error!(InvalidOperation));
    }
    school_account(db, &id).await?;
    let until = iso8601_timestamp::Timestamp::now_utc()
        + iso8601_timestamp::Duration::hours(data.hours);
    let mut user = db.fetch_user(&id).await?;
    // Reason is None so the database does not send the suspension email
    user.suspend(db, Some(until), None).await?;
    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<bson::Document>("users")
            .update_one(doc! { "_id": &id }, doc! { "$set": { "admin_suspend_reason": &reason } })
            .await
            .map_err(|_| create_error!(InternalError))?;
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
    record_action(db, data.message_id.as_deref(), &id, "suspended", &panel.email, &format!("{}h: {}", data.hours, reason)).await;
    audit(db, &panel.email, "suspend", &id, &format!("{}h: {}", data.hours, reason)).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Unsuspend School Account
///
/// Staff action from the panel. Limited to school accounts and recorded in the audit log.
#[openapi(tag = "Admin")]
#[post("/safety/users/<id>/unsuspend", data = "<data>")]
pub async fn unsuspend_school_account(
    panel: PanelUser,
    db: &State<Database>,
    id: String,
    data: Option<Json<PanelFlagRef>>,
) -> Result<Json<serde_json::Value>> {
    let message_id = data.and_then(|d| d.message_id.clone());
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
    record_action(db, message_id.as_deref(), &id, "unsuspended", &panel.email, "").await;
    audit(db, &panel.email, "unsuspend", &id, "").await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct PanelBanData {
    /// The flag the action was taken from, if any
    pub message_id: Option<String>,
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
    let data_message_id = data.as_ref().and_then(|d| d.message_id.clone());
    let reason = data
        .and_then(|d| d.reason.clone())
        .map(|r| r.trim().to_string())
        .unwrap_or_default();
    if reason.is_empty() || reason.len() > 500 {
        return Err(create_error!(InvalidOperation));
    }

    // Disabling the account through the model removes its active sessions
    let mut account = db.fetch_account(&id).await?;
    account.disable(db).await?;

    if let Database::MongoDb(mongo) = db.inner() {
        let set = doc! { "flags": 4i32, "admin_ban_reason": &reason };
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
    record_action(db, data_message_id.as_deref(), &id, "banned", &panel.email, &reason).await;
    audit(db, &panel.email, "ban", &id, &reason).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Adds an action to the flag it was taken from, so resolved cards can show what was done.
/// Without a message id it goes on every flag for that account.
async fn record_action(db: &Database, message_id: Option<&str>, author_id: &str, action: &str, actor: &str, detail: &str) {
    let Database::MongoDb(mongo) = db else {
        return;
    };
    let entry = bson::to_bson(&doc! {
        "action": action,
        "actor": actor,
        "at": chrono::Utc::now().to_rfc3339(),
        "detail": detail,
    })
    .unwrap_or_default();
    let filter = match message_id {
        Some(id) => doc! { "message_id": id },
        None => doc! { "author_id": author_id },
    };
    let result = mongo
        .col::<bson::Document>("safety_flags")
        .update_many(filter, doc! { "$push": { "actions": entry } })
        .await;
    if let Err(err) = result {
        log::error!("Failed to record flag action {action}: {err:?}");
    }
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
    panel: PanelUser,
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
    record_action(db, Some(&message_id), "", "resolved", &panel.email, "").await;

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize, JsonSchema, Debug)]
pub struct PanelWarnData {
    /// The flag the action was taken from, if any
    pub message_id: Option<String>,
    /// What the warning is for. Shown to the user with their warnings. Required.
    pub reason: String,
}

/// # Warn School Account
///
/// Staff action from the panel. Adds a warning to the account. No email is sent.
#[openapi(tag = "Admin")]
#[post("/safety/users/<id>/warn", data = "<data>")]
pub async fn warn_school_account(
    panel: PanelUser,
    db: &State<Database>,
    id: String,
    data: Json<PanelWarnData>,
) -> Result<Json<serde_json::Value>> {
    school_account(db, &id).await?;
    let reason = data.reason.trim().to_string();
    if reason.is_empty() || reason.len() > 500 {
        return Err(create_error!(InvalidOperation));
    }
    if let Database::MongoDb(mongo) = db.inner() {
        let warning = bson::to_bson(&doc! {
            "reason": &reason,
            "created_at": chrono::Utc::now().to_rfc3339(),
        })
        .unwrap_or_default();
        mongo
            .col::<bson::Document>("users")
            .update_one(doc! { "_id": &id }, doc! { "$push": { "admin_warnings": warning } })
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    record_action(db, data.message_id.as_deref(), &id, "warned", &panel.email, &reason).await;
    audit(db, &panel.email, "warn", &id, &reason).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Fixed text for a crisis notice. The web client shows its own translated version, this is the fallback.
const CRISIS_NOTICE: &str = "You are not alone. If you are thinking about hurting yourself, you can call or text 988 (US) at any time to talk to someone. If you are in danger right now, call your local emergency number.";

#[derive(Deserialize, JsonSchema, Debug)]
pub struct PanelCrisisData {
    /// The flag the notice was sent from, if any
    pub message_id: Option<String>,
    /// Staff-only note. Never shown to the user.
    pub note: Option<String>,
}

/// # Send Crisis Notice
///
/// Staff action from the panel. Shows the user a crisis support notice the next time their client checks.
/// The notice text is fixed on the server. Any note is kept for staff and never sent to the user.
#[openapi(tag = "Admin")]
#[post("/safety/users/<id>/crisis-notice", data = "<data>")]
pub async fn crisis_notice_school_account(
    panel: PanelUser,
    db: &State<Database>,
    id: String,
    data: Json<PanelCrisisData>,
) -> Result<Json<serde_json::Value>> {
    school_account(db, &id).await?;
    let note = data.note.as_deref().unwrap_or_default().trim().to_string();
    if note.len() > 500 {
        return Err(create_error!(InvalidOperation));
    }
    if let Database::MongoDb(mongo) = db.inner() {
        let notice = bson::to_bson(&doc! {
            "kind": "crisis",
            "reason": CRISIS_NOTICE,
            "staff_note": &note,
            "created_at": chrono::Utc::now().to_rfc3339(),
        })
        .unwrap_or_default();
        mongo
            .col::<bson::Document>("users")
            .update_one(doc! { "_id": &id }, doc! { "$push": { "admin_warnings": notice } })
            .await
            .map_err(|_| create_error!(InternalError))?;
    }
    record_action(db, data.message_id.as_deref(), &id, "crisis notice sent", &panel.email, &note).await;
    audit(db, &panel.email, "crisis_notice", &id, &note).await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// # Reopen Safety Flag
///
/// Marks a resolved flag as open again so it returns to the queue.
#[openapi(tag = "Admin")]
#[post("/safety/flags/<message_id>/reopen")]
pub async fn reopen_flag(
    panel: PanelUser,
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
            doc! { "$set": { "reviewed": false }, "$unset": { "resolved_by": "" as &str } },
        )
        .await
        .map_err(|_| create_error!(InternalError))?;
    record_action(db, Some(&message_id), "", "reopened", &panel.email, "").await;
    audit(db, &panel.email, "reopen", &message_id, "").await;
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

/// Live panel feed. Sends the queue, and the open message's context if the page is watching one.
/// Pushes only when something changes, checked once a second on the server.
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
            // The message the page has open, set by the client with {"watch": "<message id>"} or null
            let mut watch: Option<String> = None;
            loop {
                let flags = school_flags(&db).await.unwrap_or_default();
                let context = match &watch {
                    Some(id) => flag_context_data(&db, id).await.ok(),
                    None => None,
                };
                let (scan_done, scan_total) = crate::util::safety::scan_progress();
                let payload = serde_json::json!({
                    "flags": flags,
                    "watch": watch,
                    "context": context,
                    "scanner": { "done": scan_done, "total": scan_total },
                })
                .to_string();
                if payload != last {
                    if stream.send(Message::Text(payload.clone())).await.is_err() {
                        break;
                    }
                    last = payload;
                }
                // Wait up to a second for a client message or close; a timeout just means check again
                match tokio::time::timeout(std::time::Duration::from_secs(1), stream.next()).await {
                    Ok(None) | Ok(Some(Err(_))) => break,
                    Ok(Some(Ok(Message::Text(text)))) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                            watch = v["watch"].as_str().map(|id| id.to_string());
                        }
                    }
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
