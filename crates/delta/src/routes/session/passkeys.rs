//! WebAuthn passkey registration and authentication.
use base64::Engine;
use futures::TryStreamExt;
use iso8601_timestamp::Timestamp;
use revolt_database::{
    mongodb::bson::{doc, DateTime, Document},
    Database, Session, AMQP,
};
use revolt_models::v0;
use revolt_result::{create_error, Result};
use rocket::{
    http::{Cookie, CookieJar, SameSite},
    serde::json::Json,
    State,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ulid::Ulid;
use url::Url;
use webauthn_rs::prelude::*;

const PASSKEYS: &str = "passkeys";
const CHALLENGES: &str = "passkey_challenges";
const CHALLENGE_LIFETIME_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Serialize, Deserialize)]
struct PasskeyRecord {
    #[serde(rename = "_id")]
    id: String,
    user_id: String,
    label: String,
    credential: String,
}

#[derive(Serialize, Deserialize)]
struct ChallengeRecord {
    #[serde(rename = "_id")]
    id: String,
    user_id: String,
    purpose: String,
    state: String,
    friendly_name: Option<String>,
    expires_at: DateTime,
}

#[derive(Deserialize)]
pub struct BeginLogin {
    email: String,
    #[serde(default)]
    friendly_name: Option<String>,
}

#[derive(Deserialize)]
pub struct RegisterStart {
    password: String,
}

#[derive(Deserialize)]
pub struct DeletePasskey {
    password: String,
}

#[derive(Deserialize)]
pub struct FinishRegistration {
    challenge_id: String,
    credential: RegisterPublicKeyCredential,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
pub struct FinishLogin {
    challenge_id: String,
    credential: PublicKeyCredential,
}

fn webauthn() -> Result<Webauthn> {
    let rp_id =
        std::env::var("TAILSTALK2_WEBAUTHN_RP_ID").unwrap_or_else(|_| "tailstalk2.com".to_owned());
    let origin = std::env::var("TAILSTALK2_WEBAUTHN_ORIGIN")
        .unwrap_or_else(|_| "https://tailstalk2.com".to_owned());
    let origin = Url::parse(&origin).map_err(|_| create_error!(InternalError))?;
    WebauthnBuilder::new(&rp_id, &origin)
        .and_then(|builder| builder.rp_name("TailsTalk 2").build())
        .map_err(|_| create_error!(InternalError))
}

fn stable_user_uuid(account_id: &str) -> uuid::Uuid {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("tailstalk2-passkey:{account_id}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

async fn save_challenge(db: &Database, record: ChallengeRecord) -> Result<()> {
    match db {
        Database::MongoDb(mongo) => {
            let collection = mongo.col::<Document>(CHALLENGES);
            collection
                .delete_many(doc! { "expires_at": { "$lt": DateTime::now() } })
                .await
                .map_err(|_| create_error!(InternalError))?;
            collection
                .insert_one(
                    revolt_database::mongodb::bson::to_document(&record)
                        .map_err(|_| create_error!(InternalError))?,
                )
                .await
                .map(|_| ())
                .map_err(|_| create_error!(InternalError))
        }
        _ => Err(create_error!(InternalError)),
    }
}

async fn consume_challenge(db: &Database, id: &str, purpose: &str) -> Result<ChallengeRecord> {
    let record = match db {
        Database::MongoDb(mongo) => mongo
            .col::<ChallengeRecord>(CHALLENGES)
            .find_one_and_delete(doc! { "_id": id, "purpose": purpose })
            .await
            .map_err(|_| create_error!(InternalError))?,
        _ => return Err(create_error!(InternalError)),
    }
    .ok_or_else(|| create_error!(InvalidToken))?;
    if record.expires_at.timestamp_millis() < DateTime::now().timestamp_millis() {
        return Err(create_error!(InvalidToken));
    }
    Ok(record)
}

async fn credentials(db: &Database, user_id: &str) -> Result<Vec<(PasskeyRecord, Passkey)>> {
    let records = match db {
        Database::MongoDb(mongo) => mongo
            .col::<PasskeyRecord>(PASSKEYS)
            .find(doc! { "user_id": user_id })
            .await
            .map_err(|_| create_error!(InternalError))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|_| create_error!(InternalError))?,
        _ => return Err(create_error!(InternalError)),
    };
    records
        .into_iter()
        .map(|record| {
            let passkey = serde_json::from_str(&record.credential)
                .map_err(|_| create_error!(InternalError))?;
            Ok((record, passkey))
        })
        .collect()
}

#[openapi(tag = "Session")]
#[post("/passkeys/register/start", data = "<data>")]
pub async fn register_start(
    db: &State<Database>,
    session: Session,
    data: Json<Value>,
) -> Result<Json<Value>> {
    let data: RegisterStart =
        serde_json::from_value(data.into_inner()).map_err(|_| create_error!(InvalidToken))?;
    let account = db.fetch_account(&session.user_id).await?;
    account.verify_password(&data.password)?;
    let user = db.fetch_user(&session.user_id).await?;
    let credentials = credentials(db, &session.user_id).await?;
    let excluded = credentials
        .iter()
        .map(|(_, passkey)| passkey.cred_id().clone())
        .collect();
    let (options, state) = webauthn()?
        .start_passkey_registration(
            stable_user_uuid(&account.id),
            &account.email_normalised,
            user.display_name.as_deref().unwrap_or(&user.username),
            Some(excluded),
        )
        .map_err(|_| create_error!(InvalidToken))?;
    let challenge_id = Ulid::new().to_string();
    save_challenge(
        db,
        ChallengeRecord {
            id: challenge_id.clone(),
            user_id: session.user_id,
            purpose: "register".to_owned(),
            state: serde_json::to_string(&state).map_err(|_| create_error!(InternalError))?,
            friendly_name: None,
            expires_at: DateTime::from_millis(
                DateTime::now().timestamp_millis() + CHALLENGE_LIFETIME_MS,
            ),
        },
    )
    .await?;
    Ok(Json(
        json!({ "challenge_id": challenge_id, "publicKey": options }),
    ))
}

#[openapi(tag = "Session")]
#[post("/passkeys/register/finish", data = "<data>")]
pub async fn register_finish(
    db: &State<Database>,
    session: Session,
    data: Json<Value>,
) -> Result<Json<Value>> {
    let data: FinishRegistration =
        serde_json::from_value(data.into_inner()).map_err(|_| create_error!(InvalidToken))?;
    let challenge = consume_challenge(db, &data.challenge_id, "register").await?;
    if challenge.user_id != session.user_id {
        return Err(create_error!(InvalidToken));
    }
    let state: PasskeyRegistration =
        serde_json::from_str(&challenge.state).map_err(|_| create_error!(InvalidToken))?;
    let passkey = webauthn()?
        .finish_passkey_registration(&data.credential, &state)
        .map_err(|_| create_error!(InvalidToken))?;
    let id = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(passkey.cred_id());
    let label = data.name.unwrap_or_default();
    let label = label.trim();
    let record = PasskeyRecord {
        id: id.clone(),
        user_id: session.user_id,
        label: if label.is_empty() { "Passkey" } else { label }
            .chars()
            .take(80)
            .collect(),
        credential: serde_json::to_string(&passkey).map_err(|_| create_error!(InternalError))?,
    };
    match &**db {
        Database::MongoDb(mongo) => {
            if mongo
                .col::<Document>(PASSKEYS)
                .find_one(doc! { "_id": &id })
                .await
                .map_err(|_| create_error!(InternalError))?
                .is_some()
            {
                return Err(create_error!(InvalidToken));
            }
            mongo
                .col::<PasskeyRecord>(PASSKEYS)
                .insert_one(record)
                .await
                .map_err(|_| create_error!(InternalError))?;
        }
        _ => return Err(create_error!(InternalError)),
    }
    Ok(Json(json!({ "id": id })))
}

#[openapi(tag = "Session")]
#[get("/passkeys")]
pub async fn list(db: &State<Database>, session: Session) -> Result<Json<Value>> {
    let records = credentials(db, &session.user_id).await?;
    Ok(Json(json!(records
        .into_iter()
        .map(|(record, passkey)| json!({
            "id": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(passkey.cred_id()),
            "name": record.label,
        }))
        .collect::<Vec<_>>())))
}

#[openapi(tag = "Session")]
#[delete("/passkeys/<id>", data = "<data>")]
pub async fn delete(
    db: &State<Database>,
    session: Session,
    id: String,
    data: Json<Value>,
) -> Result<Json<Value>> {
    let data: DeletePasskey =
        serde_json::from_value(data.into_inner()).map_err(|_| create_error!(InvalidToken))?;
    let account = db.fetch_account(&session.user_id).await?;
    account.verify_password(&data.password)?;
    match &**db {
        Database::MongoDb(mongo) => {
            let result = mongo
                .col::<Document>(PASSKEYS)
                .delete_one(doc! { "_id": id, "user_id": session.user_id })
                .await
                .map_err(|_| create_error!(InternalError))?;
            if result.deleted_count == 0 {
                return Err(create_error!(InvalidToken));
            }
        }
        _ => return Err(create_error!(InternalError)),
    }
    Ok(Json(json!({ "ok": true })))
}

#[openapi(tag = "Session")]
#[post("/passkeys/login/start", data = "<data>")]
pub async fn login_start(db: &State<Database>, data: Json<Value>) -> Result<Json<Value>> {
    let data: BeginLogin =
        serde_json::from_value(data.into_inner()).map_err(|_| create_error!(InvalidToken))?;
    let email = revolt_database::util::email::normalise_email(data.email.clone());
    let account = db
        .fetch_account_by_normalised_email(&email)
        .await?
        .ok_or_else(|| create_error!(InvalidCredentials))?;
    if account.disabled
        || account
            .lockout
            .as_ref()
            .and_then(|lockout| lockout.expiry)
            .map(|expiry| expiry > Timestamp::now_utc())
            .unwrap_or(false)
    {
        return Err(create_error!(InvalidCredentials));
    }
    let candidates = credentials(db, &account.id).await?;
    let passkeys = candidates
        .iter()
        .map(|(_, passkey)| passkey.clone())
        .collect::<Vec<_>>();
    if passkeys.is_empty() {
        return Err(create_error!(InvalidCredentials));
    }
    let (options, state) = webauthn()?
        .start_passkey_authentication(&passkeys)
        .map_err(|_| create_error!(InvalidCredentials))?;
    let challenge_id = Ulid::new().to_string();
    save_challenge(
        db,
        ChallengeRecord {
            id: challenge_id.clone(),
            user_id: account.id,
            purpose: "login".to_owned(),
            state: serde_json::to_string(&state).map_err(|_| create_error!(InternalError))?,
            friendly_name: data.friendly_name.clone(),
            expires_at: DateTime::from_millis(
                DateTime::now().timestamp_millis() + CHALLENGE_LIFETIME_MS,
            ),
        },
    )
    .await?;
    Ok(Json(
        json!({ "challenge_id": challenge_id, "publicKey": options }),
    ))
}

#[openapi(tag = "Session")]
#[post("/passkeys/login/finish", data = "<data>")]
pub async fn login_finish(
    db: &State<Database>,
    amqp: &State<AMQP>,
    cookies: &CookieJar<'_>,
    data: Json<Value>,
) -> Result<Json<Value>> {
    let data: FinishLogin =
        serde_json::from_value(data.into_inner()).map_err(|_| create_error!(InvalidToken))?;
    let challenge = consume_challenge(db, &data.challenge_id, "login").await?;
    let state: PasskeyAuthentication =
        serde_json::from_str(&challenge.state).map_err(|_| create_error!(InvalidToken))?;
    let mut records = credentials(db, &challenge.user_id).await?;
    let result = webauthn()?
        .finish_passkey_authentication(&data.credential, &state)
        .map_err(|_| create_error!(InvalidCredentials))?;
    for (record, passkey) in &mut records {
        if passkey.update_credential(&result).is_some() {
            record.credential =
                serde_json::to_string(passkey).map_err(|_| create_error!(InternalError))?;
        }
    }
    if let Database::MongoDb(mongo) = &**db {
        for (record, _) in records {
            mongo
                .col::<PasskeyRecord>(PASSKEYS)
                .replace_one(doc! { "_id": &record.id }, record)
                .await
                .map_err(|_| create_error!(InternalError))?;
        }
    }
    let account = db.fetch_account(&challenge.user_id).await?;
    if account.disabled {
        return Ok(Json(json!(v0::ResponseLogin::Disabled {
            user_id: account.id,
            reason: None
        })));
    }
    if account.mfa.is_active() {
        let mut ticket = revolt_database::MFATicket::new(account.id, false);
        ticket.populate(&account.mfa).await;
        ticket.save(db).await?;
        return Ok(Json(json!(v0::ResponseLogin::MFA {
            ticket: ticket.token,
            allowed_methods: account
                .mfa
                .get_methods()
                .into_iter()
                .map(Into::into)
                .collect(),
        })));
    }
    let session = account
        .create_session(
            db,
            challenge
                .friendly_name
                .unwrap_or_else(|| "Passkey".to_owned()),
        )
        .await?;
    if let Ok(user) = db.fetch_user(&account.id).await {
        let _ = amqp
            .generic_message(
                &user,
                "New TailsTalk 2 login".to_owned(),
                format!("A new session signed in as {}.", session.name),
                None,
            )
            .await;
    }
    cookies.add(
        Cookie::build(("__Host-tailstalk_session", session.token.clone()))
            .http_only(true)
            .secure(true)
            .same_site(SameSite::Lax)
            .path("/")
            .build(),
    );
    Ok(Json(json!(v0::ResponseLogin::Success(session.into()))))
}
