//! OAuth 2.1 authorization-code flow with PKCE.
//!
//! Tokens and one-time values are opaque and are stored only as SHA-256
//! digests. The implementation intentionally supports authorization_code and
//! refresh_token only; implicit and password grants are not implemented.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use once_cell::sync::Lazy;
use rand::{rngs::OsRng, RngCore};
use revolt_database::{
    mongodb::{
        bson::{doc, DateTime, Document},
        options::ReturnDocument,
    },
    util::permissions::DatabasePermissionQuery,
    Database, Session, User,
};
use revolt_permissions::{calculate_channel_permissions, calculate_server_permissions};
use revolt_result::Result as RevoltResult;
use rocket::{
    form::FromForm,
    http::Status,
    request::{FromRequest, Outcome},
    response::{content::RawHtml, Redirect},
    serde::json::Json,
    State,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

const AUTHORIZATION_CODE_TTL: Duration = Duration::from_secs(300);
const ACCESS_TOKEN_TTL: Duration = Duration::from_secs(600);
const REFRESH_TOKEN_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const SCOPES: [&str; 4] = ["identify", "servers", "server_members", "permissions"];
static RATE_LIMITS: Lazy<Mutex<HashMap<String, (Instant, u32)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Serialize, JsonSchema, Clone)]
pub struct OAuthError {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
}

type OAuthResult<T> = std::result::Result<Json<T>, (Status, Json<OAuthError>)>;

fn error(status: Status, code: &str, description: &str) -> (Status, Json<OAuthError>) {
    (
        status,
        Json(OAuthError {
            error: code.to_owned(),
            error_description: Some(description.to_owned()),
        }),
    )
}

fn random_value(bytes: usize) -> String {
    let mut value = vec![0; bytes];
    OsRng.fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn digest(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

fn constant_time_equal(left: &str, right: &str) -> bool {
    let mut result = left.len() ^ right.len();
    for (a, b) in left.as_bytes().iter().zip(right.as_bytes()) {
        result |= (*a ^ *b) as usize;
    }
    result == 0
}

fn pkce_matches(verifier: &str, challenge: &str) -> bool {
    let computed = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    constant_time_equal(&computed, challenge)
}

fn now() -> DateTime {
    DateTime::now()
}

fn expires_after(duration: Duration) -> DateTime {
    DateTime::from_system_time(SystemTime::now() + duration)
}

fn validate_redirect_uri(uri: &str) -> bool {
    Url::parse(uri)
        .map(|url| {
            url.scheme() == "https"
                || (url.scheme() == "http" && url.host_str() == Some("localhost"))
        })
        .unwrap_or(false)
}

fn validate_scopes(requested: &str, allowed: &[String]) -> Option<Vec<String>> {
    let scopes: Vec<String> = requested.split_whitespace().map(str::to_owned).collect();
    if scopes.is_empty()
        || scopes
            .iter()
            .any(|scope| !SCOPES.contains(&scope.as_str()) || !allowed.iter().any(|x| x == scope))
    {
        return None;
    }
    Some(scopes)
}

fn redirect_error(uri: &str, state: &str, code: &str, description: &str) -> Redirect {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("error", code)
        .append_pair("error_description", description)
        .append_pair("state", state)
        .finish();
    Redirect::to(format!("{uri}?{query}"))
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(FromForm, Debug, JsonSchema)]
pub struct AuthorizeQuery<'r> {
    response_type: &'r str,
    client_id: &'r str,
    redirect_uri: &'r str,
    scope: &'r str,
    state: &'r str,
    code_challenge: Option<&'r str>,
    code_challenge_method: Option<&'r str>,
}

#[derive(FromForm, Debug, JsonSchema)]
pub struct ConsentForm<'r> {
    request_id: &'r str,
    csrf: &'r str,
    decision: &'r str,
}

#[derive(FromForm, Serialize, Deserialize, JsonSchema, Debug)]
pub struct TokenRequest {
    pub grant_type: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: &'static str,
    pub expires_in: u64,
    pub refresh_token: String,
    pub scope: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug)]
pub struct ApplicationCreate {
    pub name: String,
    pub redirect_uris: Vec<String>,
    pub allowed_scopes: Vec<String>,
    pub public: bool,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct ApplicationResponse {
    pub client_id: String,
    pub name: String,
    pub owner_id: String,
    pub redirect_uris: Vec<String>,
    pub allowed_scopes: Vec<String>,
    pub public: bool,
    pub revoked: bool,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct UserInfo {
    pub sub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<ServerInfo>>,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct ServerInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<MemberInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<Vec<ChannelPermissionInfo>>,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct MemberInfo {
    pub roles: Vec<String>,
    pub joined_at: String,
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct ChannelPermissionInfo {
    pub id: String,
    pub permissions: u64,
}

pub struct BearerToken(pub String);

pub struct OAuthRateLimit;

#[rocket::async_trait]
impl<'r> FromRequest<'r> for OAuthRateLimit {
    type Error = ();
    async fn from_request(request: &'r rocket::Request<'_>) -> Outcome<Self, Self::Error> {
        let key = request
            .client_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        let mut limits = RATE_LIMITS
            .lock()
            .expect("OAuth rate limiter mutex poisoned");
        let entry = limits.entry(key).or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= Duration::from_secs(60) {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
        if entry.1 > 60 {
            Outcome::Error((Status::TooManyRequests, ()))
        } else {
            Outcome::Success(Self)
        }
    }
}

impl<'r> revolt_rocket_okapi::request::OpenApiFromRequest<'r> for OAuthRateLimit {
    fn from_request_input(
        _gen: &mut revolt_rocket_okapi::gen::OpenApiGenerator,
        _name: String,
        _required: bool,
    ) -> revolt_rocket_okapi::Result<revolt_rocket_okapi::request::RequestHeaderInput> {
        Ok(revolt_rocket_okapi::request::RequestHeaderInput::None)
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for BearerToken {
    type Error = ();
    async fn from_request(request: &'r rocket::Request<'_>) -> Outcome<Self, Self::Error> {
        match request
            .headers()
            .get_one("Authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
        {
            Some(token) if !token.is_empty() => Outcome::Success(Self(token.to_owned())),
            _ => Outcome::Error((Status::Unauthorized, ())),
        }
    }
}

impl<'r> revolt_rocket_okapi::request::OpenApiFromRequest<'r> for BearerToken {
    fn from_request_input(
        _gen: &mut revolt_rocket_okapi::gen::OpenApiGenerator,
        _name: String,
        _required: bool,
    ) -> revolt_rocket_okapi::Result<revolt_rocket_okapi::request::RequestHeaderInput> {
        use revolt_rocket_okapi::revolt_okapi::openapi3::{SecurityScheme, SecuritySchemeData};
        let mut requirements = schemars::Map::new();
        requirements.insert("OAuth Bearer".to_owned(), vec![]);
        Ok(revolt_rocket_okapi::request::RequestHeaderInput::Security(
            "OAuth Bearer".to_owned(),
            SecurityScheme {
                data: SecuritySchemeData::Http {
                    scheme: "bearer".to_owned(),
                    bearer_format: Some("opaque".to_owned()),
                },
                description: Some("OAuth 2.1 bearer access token".to_owned()),
                extensions: schemars::Map::new(),
            },
            requirements,
        ))
    }
}

fn app_response(doc: &Document, secret: Option<String>) -> Option<ApplicationResponse> {
    Some(ApplicationResponse {
        client_id: doc.get_str("_id").ok()?.to_owned(),
        name: doc.get_str("name").ok()?.to_owned(),
        owner_id: doc.get_str("owner_id").ok()?.to_owned(),
        redirect_uris: doc
            .get_array("redirect_uris")
            .ok()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        allowed_scopes: doc
            .get_array("allowed_scopes")
            .ok()?
            .iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        public: doc.get_bool("public").unwrap_or(false),
        revoked: doc.get_bool("revoked").unwrap_or(false),
        created_at: doc
            .get_datetime("created_at")
            .ok()?
            .try_to_rfc3339_string()
            .ok()?,
        client_secret: secret,
    })
}

async fn application(db: &Database, client_id: &str) -> RevoltResult<Option<Document>> {
    match db {
        Database::MongoDb(mongo) => Ok(mongo
            .col::<Document>("oauth_applications")
            .find_one(doc! { "_id": client_id })
            .await
            .map_err(|_| revolt_result::create_error!(InternalError))?),
        Database::Reference(_) => Ok(None),
    }
}

async fn bearer_user(db: &Database, token: &str) -> RevoltResult<Option<(Document, User)>> {
    let doc = match db {
        Database::MongoDb(mongo) => mongo.col::<Document>("oauth_access_tokens").find_one(doc! { "token_hash": digest(token), "revoked": false, "expires_at": { "$gt": now() } }).await.map_err(|_| revolt_result::create_error!(InternalError))?,
        Database::Reference(_) => None,
    };
    if let Some(doc) = doc {
        let user_id = doc
            .get_str("user_id")
            .map_err(|_| revolt_result::create_error!(InvalidToken))?
            .to_owned();
        Ok(Some((doc, db.fetch_user(&user_id).await?)))
    } else {
        Ok(None)
    }
}

/// OAuth authorization endpoint. It requires the existing Tailstalk session
/// guard and renders a consent form; it never accepts a password.
#[openapi(tag = "OAuth")]
#[get("/authorize?<query..>")]
pub async fn authorize(
    db: &State<Database>,
    _rate: OAuthRateLimit,
    session: Session,
    query: AuthorizeQuery<'_>,
) -> std::result::Result<RawHtml<String>, (Status, Json<OAuthError>)> {
    if query.response_type != "code" || query.state.is_empty() {
        return Err(error(
            Status::BadRequest,
            "invalid_request",
            "response_type=code and state are required",
        ));
    }
    let app = application(db, query.client_id)
        .await
        .map_err(|_| {
            error(
                Status::InternalServerError,
                "server_error",
                "OAuth storage unavailable",
            )
        })?
        .ok_or_else(|| error(Status::BadRequest, "invalid_client", "Unknown client"))?;
    if app.get_bool("revoked").unwrap_or(false) {
        return Err(error(
            Status::BadRequest,
            "invalid_client",
            "Application revoked",
        ));
    }
    let redirect_uris: Vec<&str> = app
        .get_array("redirect_uris")
        .map_err(|_| {
            error(
                Status::BadRequest,
                "invalid_client",
                "Application is malformed",
            )
        })?
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    if !redirect_uris.iter().any(|uri| *uri == query.redirect_uri)
        || !validate_redirect_uri(query.redirect_uri)
    {
        return Err(error(
            Status::BadRequest,
            "invalid_request",
            "redirect_uri does not exactly match the application allowlist",
        ));
    }
    let allowed: Vec<String> = app
        .get_array("allowed_scopes")
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let scopes = validate_scopes(query.scope, &allowed).ok_or_else(|| {
        error(
            Status::BadRequest,
            "invalid_scope",
            "Requested scopes are not allowed",
        )
    })?;
    let is_public = app.get_bool("public").unwrap_or(false);
    if is_public && (query.code_challenge.is_none() || query.code_challenge_method != Some("S256"))
    {
        return Err(error(
            Status::BadRequest,
            "invalid_request",
            "Public clients must use PKCE S256",
        ));
    }
    if let Some(method) = query.code_challenge_method {
        if method != "S256" {
            return Err(error(
                Status::BadRequest,
                "invalid_request",
                "Only PKCE S256 is supported",
            ));
        }
    }
    if query
        .code_challenge
        .map(|challenge| challenge.len() > 256)
        .unwrap_or(false)
    {
        return Err(error(
            Status::BadRequest,
            "invalid_request",
            "PKCE challenge is too long",
        ));
    }
    let request_id = random_value(24);
    let csrf = random_value(32);
    if let Database::MongoDb(mongo) = db.inner() {
        mongo.col::<Document>("oauth_consent_requests").insert_one(doc! {
            "_id": &request_id, "csrf_hash": digest(&csrf), "client_id": query.client_id,
            "user_id": &session.user_id, "redirect_uri": query.redirect_uri, "state": query.state,
            "scope": scopes.join(" "), "code_challenge": query.code_challenge, "expires_at": expires_after(Duration::from_secs(300)), "used": false
        }).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?;
    }
    let user = db.fetch_user(&session.user_id).await.map_err(|_| {
        error(
            Status::InternalServerError,
            "server_error",
            "User unavailable",
        )
    })?;
    let scope_list = scopes
        .iter()
        .map(|s| format!("<li>{}</li>", html_escape(s)))
        .collect::<String>();
    Ok(RawHtml(format!("<!doctype html><title>Authorize {}</title><main><h1>{} requests access</h1><p>Signed in as <strong>{}</strong>. Review the requested access:</p><ul>{}</ul><form method=post action=/oauth/authorize><input type=hidden name=request_id value=\"{}\"><input type=hidden name=csrf value=\"{}\"><button name=decision value=deny>Deny</button><button name=decision value=approve>Allow</button></form></main>", html_escape(query.client_id), html_escape(app.get_str("name").unwrap_or("Application")), html_escape(&user.username), scope_list, html_escape(&request_id), html_escape(&csrf))))
}

/// Submit the consent form generated by `GET /oauth/authorize`.
#[openapi(tag = "OAuth")]
#[post("/authorize", data = "<form>")]
pub async fn authorize_consent(
    db: &State<Database>,
    _rate: OAuthRateLimit,
    session: Session,
    form: rocket::form::Form<ConsentForm<'_>>,
) -> std::result::Result<Redirect, (Status, Json<OAuthError>)> {
    let request = match db.inner() {
        Database::MongoDb(mongo) => mongo.col::<Document>("oauth_consent_requests").find_one_and_update(
            doc! { "_id": form.request_id, "user_id": &session.user_id, "csrf_hash": digest(form.csrf), "used": false, "expires_at": { "$gt": now() } },
            doc! { "$set": { "used": true } },
        ).return_document(ReturnDocument::After).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?,
        Database::Reference(_) => None,
    }.ok_or_else(|| error(Status::BadRequest, "invalid_request", "Expired or invalid consent request"))?;
    let redirect_uri = request.get_str("redirect_uri").unwrap_or_default();
    let state = request.get_str("state").unwrap_or_default();
    if form.decision != "approve" {
        return Ok(redirect_error(
            redirect_uri,
            state,
            "access_denied",
            "The user denied the request",
        ));
    }
    let code = random_value(32);
    if let Database::MongoDb(mongo) = db.inner() {
        mongo.col::<Document>("oauth_authorization_codes").insert_one(doc! {
            "code_hash": digest(&code), "client_id": request.get_str("client_id").unwrap_or_default(), "user_id": &session.user_id,
            "redirect_uri": redirect_uri, "scope": request.get_str("scope").unwrap_or_default(),
            "code_challenge": request.get_str("code_challenge").ok(), "expires_at": expires_after(AUTHORIZATION_CODE_TTL), "used": false
        }).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?;
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("code", &code)
        .append_pair("state", state)
        .finish();
    Ok(Redirect::to(format!("{redirect_uri}?{query}")))
}

async fn exchange_code(db: &Database, req: &TokenRequest) -> OAuthResult<TokenResponse> {
    let code = req
        .code
        .as_deref()
        .ok_or_else(|| error(Status::BadRequest, "invalid_request", "code is required"))?;
    let client_id = req.client_id.as_deref().ok_or_else(|| {
        error(
            Status::BadRequest,
            "invalid_request",
            "client_id is required",
        )
    })?;
    let app = application(db, client_id)
        .await
        .map_err(|_| {
            error(
                Status::InternalServerError,
                "server_error",
                "OAuth storage unavailable",
            )
        })?
        .ok_or_else(|| error(Status::Unauthorized, "invalid_client", "Invalid client"))?;
    if app.get_bool("revoked").unwrap_or(false) {
        return Err(error(
            Status::Unauthorized,
            "invalid_client",
            "Application revoked",
        ));
    }
    if !app.get_bool("public").unwrap_or(false) {
        let secret = req.client_secret.as_deref().ok_or_else(|| {
            error(
                Status::Unauthorized,
                "invalid_client",
                "client_secret is required",
            )
        })?;
        if !constant_time_equal(
            &digest(secret),
            app.get_str("secret_hash").unwrap_or_default(),
        ) {
            return Err(error(
                Status::Unauthorized,
                "invalid_client",
                "Invalid client credentials",
            ));
        }
    }
    let code_doc = match db {
        Database::MongoDb(mongo) => mongo.col::<Document>("oauth_authorization_codes").find_one_and_update(
            doc! { "code_hash": digest(code), "client_id": client_id, "used": false, "expires_at": { "$gt": now() } }, doc! { "$set": { "used": true } },
        ).return_document(ReturnDocument::After).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?,
        Database::Reference(_) => None,
    }.ok_or_else(|| error(Status::BadRequest, "invalid_grant", "Invalid, expired, or reused authorization code"))?;
    if req.redirect_uri.as_deref() != code_doc.get_str("redirect_uri").ok() {
        return Err(error(
            Status::BadRequest,
            "invalid_grant",
            "redirect_uri mismatch",
        ));
    }
    if let Ok(challenge) = code_doc.get_str("code_challenge") {
        let verifier = req.code_verifier.as_deref().ok_or_else(|| {
            error(
                Status::BadRequest,
                "invalid_grant",
                "code_verifier is required",
            )
        })?;
        if !pkce_matches(verifier, challenge) {
            return Err(error(
                Status::BadRequest,
                "invalid_grant",
                "PKCE verification failed",
            ));
        }
    }
    issue_tokens(
        db,
        client_id,
        code_doc.get_str("user_id").unwrap_or_default(),
        code_doc.get_str("scope").unwrap_or_default(),
        None,
    )
    .await
}

async fn issue_tokens(
    db: &Database,
    client_id: &str,
    user_id: &str,
    scope: &str,
    family_id: Option<&str>,
) -> OAuthResult<TokenResponse> {
    let access = random_value(32);
    let refresh = random_value(48);
    let family = family_id
        .map(str::to_owned)
        .unwrap_or_else(|| random_value(24));
    if let Database::MongoDb(mongo) = db {
        mongo.col::<Document>("oauth_access_tokens").insert_one(doc! { "token_hash": digest(&access), "client_id": client_id, "user_id": user_id, "scope": scope, "expires_at": expires_after(ACCESS_TOKEN_TTL), "revoked": false }).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?;
        mongo.col::<Document>("oauth_refresh_tokens").insert_one(doc! { "token_hash": digest(&refresh), "family_id": family, "client_id": client_id, "user_id": user_id, "scope": scope, "expires_at": expires_after(REFRESH_TOKEN_TTL), "revoked": false }).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?;
    }
    Ok(Json(TokenResponse {
        access_token: access,
        token_type: "Bearer",
        expires_in: ACCESS_TOKEN_TTL.as_secs(),
        refresh_token: refresh,
        scope: scope.to_owned(),
    }))
}

#[openapi(tag = "OAuth")]
#[post("/token", data = "<request>")]
pub async fn token(
    db: &State<Database>,
    _rate: OAuthRateLimit,
    request: rocket::form::Form<TokenRequest>,
) -> OAuthResult<TokenResponse> {
    match request.grant_type.as_str() {
        "authorization_code" => exchange_code(db, &request).await,
        "refresh_token" => refresh(db, &request).await,
        _ => Err(error(
            Status::BadRequest,
            "unsupported_grant_type",
            "Only authorization_code and refresh_token are supported",
        )),
    }
}

async fn refresh(db: &Database, req: &TokenRequest) -> OAuthResult<TokenResponse> {
    let token = req.refresh_token.as_deref().ok_or_else(|| {
        error(
            Status::BadRequest,
            "invalid_request",
            "refresh_token is required",
        )
    })?;
    let client_id = req.client_id.as_deref().ok_or_else(|| {
        error(
            Status::BadRequest,
            "invalid_request",
            "client_id is required",
        )
    })?;
    let app = application(db, client_id)
        .await
        .map_err(|_| {
            error(
                Status::InternalServerError,
                "server_error",
                "OAuth storage unavailable",
            )
        })?
        .ok_or_else(|| error(Status::Unauthorized, "invalid_client", "Invalid client"))?;
    if !app.get_bool("public").unwrap_or(false) {
        let secret = req.client_secret.as_deref().ok_or_else(|| {
            error(
                Status::Unauthorized,
                "invalid_client",
                "client_secret is required",
            )
        })?;
        if !constant_time_equal(
            &digest(secret),
            app.get_str("secret_hash").unwrap_or_default(),
        ) {
            return Err(error(
                Status::Unauthorized,
                "invalid_client",
                "Invalid client credentials",
            ));
        }
    }
    let old = match db {
            Database::MongoDb(mongo) => mongo.col::<Document>("oauth_refresh_tokens").find_one_and_update(doc! { "token_hash": digest(token), "client_id": client_id, "revoked": false, "expires_at": { "$gt": now() } }, doc! { "$set": { "revoked": true } }).return_document(ReturnDocument::After).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?,
        Database::Reference(_) => None,
    };
    let old = if let Some(old) = old {
        old
    } else {
        if let Database::MongoDb(mongo) = db {
            if let Some(reused) = mongo
                .col::<Document>("oauth_refresh_tokens")
                .find_one(doc! { "token_hash": digest(token), "client_id": client_id })
                .await
                .map_err(|_| {
                    error(
                        Status::InternalServerError,
                        "server_error",
                        "OAuth storage unavailable",
                    )
                })?
            {
                if let Ok(family) = reused.get_str("family_id") {
                    mongo
                        .col::<Document>("oauth_refresh_tokens")
                        .update_many(
                            doc! { "family_id": family },
                            doc! { "$set": { "revoked": true } },
                        )
                        .await
                        .ok();
                }
            }
        }
        return Err(error(
            Status::BadRequest,
            "invalid_grant",
            "Refresh token is invalid, expired, revoked, or already used",
        ));
    };
    issue_tokens(
        db,
        client_id,
        old.get_str("user_id").unwrap_or_default(),
        old.get_str("scope").unwrap_or_default(),
        old.get_str("family_id").ok(),
    )
    .await
}

#[derive(FromForm, Deserialize, JsonSchema, Debug)]
pub struct RevokeRequest {
    pub token: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
}

#[openapi(tag = "OAuth")]
#[post("/revoke", data = "<request>")]
pub async fn revoke(
    db: &State<Database>,
    _rate: OAuthRateLimit,
    request: rocket::form::Form<RevokeRequest>,
) -> std::result::Result<Status, (Status, Json<OAuthError>)> {
    if let Some(client_id) = request.client_id.as_deref() {
        let app = application(db, client_id)
            .await
            .map_err(|_| {
                error(
                    Status::InternalServerError,
                    "server_error",
                    "OAuth storage unavailable",
                )
            })?
            .ok_or_else(|| error(Status::Unauthorized, "invalid_client", "Invalid client"))?;
        if !app.get_bool("public").unwrap_or(false) {
            let secret = request.client_secret.as_deref().ok_or_else(|| {
                error(
                    Status::Unauthorized,
                    "invalid_client",
                    "client_secret is required",
                )
            })?;
            if !constant_time_equal(
                &digest(secret),
                app.get_str("secret_hash").unwrap_or_default(),
            ) {
                return Err(error(
                    Status::Unauthorized,
                    "invalid_client",
                    "Invalid client credentials",
                ));
            }
        }
    }
    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<Document>("oauth_access_tokens")
            .update_one(
                doc! { "token_hash": digest(&request.token) },
                doc! { "$set": { "revoked": true } },
            )
            .await
            .map_err(|_| {
                error(
                    Status::InternalServerError,
                    "server_error",
                    "OAuth storage unavailable",
                )
            })?;
        mongo
            .col::<Document>("oauth_refresh_tokens")
            .update_one(
                doc! { "token_hash": digest(&request.token) },
                doc! { "$set": { "revoked": true } },
            )
            .await
            .map_err(|_| {
                error(
                    Status::InternalServerError,
                    "server_error",
                    "OAuth storage unavailable",
                )
            })?;
    }
    Ok(Status::NoContent)
}

#[openapi(tag = "OAuth")]
#[get("/userinfo")]
pub async fn userinfo(db: &State<Database>, bearer: BearerToken) -> OAuthResult<UserInfo> {
    let (token_doc, user) = bearer_user(db, &bearer.0)
        .await
        .map_err(|_| {
            error(
                Status::InternalServerError,
                "server_error",
                "OAuth storage unavailable",
            )
        })?
        .ok_or_else(|| {
            error(
                Status::Unauthorized,
                "invalid_token",
                "Invalid or expired access token",
            )
        })?;
    let scopes: HashSet<&str> = token_doc
        .get_str("scope")
        .unwrap_or_default()
        .split_whitespace()
        .collect();
    let mut response = UserInfo {
        sub: user.id.clone(),
        username: None,
        display_name: None,
        avatar: None,
        servers: None,
    };
    if scopes.contains("identify") {
        response.username = Some(user.username.clone());
        response.display_name = user.display_name.clone();
        response.avatar = user.avatar.clone().map(|avatar| avatar.id);
    }
    if scopes.contains("servers")
        || scopes.contains("server_members")
        || scopes.contains("permissions")
    {
        let memberships = db.fetch_all_memberships(&user.id).await.map_err(|_| {
            error(
                Status::InternalServerError,
                "server_error",
                "Membership lookup failed",
            )
        })?;
        let mut servers = Vec::new();
        for member in memberships {
            let server = match db.fetch_server(&member.id.server).await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let mut item = ServerInfo {
                id: server.id.clone(),
                name: server.name.clone(),
                owner: None,
                member: None,
                permissions: None,
                channels: None,
            };
            if scopes.contains("servers") {
                item.owner = Some(server.owner.clone());
            }
            if scopes.contains("server_members") {
                item.member = Some(MemberInfo {
                    roles: member.roles.clone(),
                    joined_at: member.joined_at.to_string(),
                });
            }
            if scopes.contains("permissions") {
                let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
                item.permissions = Some(calculate_server_permissions(&mut query).await.into());
                let mut channels = Vec::new();
                for channel in db
                    .fetch_channels(&server.channels)
                    .await
                    .unwrap_or_default()
                {
                    let mut query = DatabasePermissionQuery::new(db, &user)
                        .server(&server)
                        .channel(&channel);
                    channels.push(ChannelPermissionInfo {
                        id: channel.id().to_string(),
                        permissions: calculate_channel_permissions(&mut query).await.into(),
                    });
                }
                item.channels = Some(channels);
            }
            servers.push(item);
        }
        response.servers = Some(servers);
    }
    Ok(Json(response))
}

#[openapi(tag = "OAuth")]
#[get("/applications/@me")]
pub async fn applications_me(
    db: &State<Database>,
    session: Session,
) -> OAuthResult<Vec<ApplicationResponse>> {
    let apps = match db.inner() {
        Database::MongoDb(mongo) => {
            let mut cursor = mongo
                .col::<Document>("oauth_applications")
                .find(doc! { "owner_id": &session.user_id })
                .await
                .map_err(|_| {
                    error(
                        Status::InternalServerError,
                        "server_error",
                        "OAuth storage unavailable",
                    )
                })?;
            let mut apps = Vec::new();
            use futures::StreamExt;
            while let Some(Ok(doc)) = cursor.next().await {
                if let Some(app) = app_response(&doc, None) {
                    apps.push(app);
                }
            }
            apps
        }
        Database::Reference(_) => vec![],
    };
    Ok(Json(apps))
}

#[openapi(tag = "OAuth")]
#[post("/applications", data = "<request>")]
pub async fn application_create(
    db: &State<Database>,
    session: Session,
    request: Json<ApplicationCreate>,
) -> OAuthResult<ApplicationResponse> {
    if request.name.trim().is_empty()
        || request.name.len() > 100
        || request.redirect_uris.is_empty()
        || request
            .redirect_uris
            .iter()
            .any(|uri| !validate_redirect_uri(uri))
    {
        return Err(error(
            Status::BadRequest,
            "invalid_request",
            "Name and secure exact redirect URIs are required",
        ));
    }
    if request.redirect_uris.len() > 20 || request.redirect_uris.iter().any(|uri| uri.len() > 2048)
    {
        return Err(error(
            Status::BadRequest,
            "invalid_request",
            "Too many or oversized redirect URIs",
        ));
    }
    let allowed: Vec<String> = request
        .allowed_scopes
        .iter()
        .filter(|scope| SCOPES.contains(&scope.as_str()))
        .cloned()
        .collect();
    if allowed.len() != request.allowed_scopes.len() || allowed.is_empty() {
        return Err(error(
            Status::BadRequest,
            "invalid_scope",
            "All scopes must be supported",
        ));
    }
    let client_id = random_value(18);
    let secret = if request.public {
        None
    } else {
        Some(random_value(32))
    };
    let doc = doc! { "_id": &client_id, "name": request.name.trim(), "owner_id": &session.user_id, "redirect_uris": &request.redirect_uris, "allowed_scopes": &allowed, "public": request.public, "secret_hash": secret.as_deref().map(digest), "revoked": false, "created_at": now() };
    if let Database::MongoDb(mongo) = db.inner() {
        mongo
            .col::<Document>("oauth_applications")
            .insert_one(doc.clone())
            .await
            .map_err(|_| {
                error(
                    Status::InternalServerError,
                    "server_error",
                    "OAuth storage unavailable",
                )
            })?;
    } else {
        return Err(error(
            Status::NotImplemented,
            "temporarily_unavailable",
            "OAuth requires MongoDB",
        ));
    }
    Ok(Json(
        app_response(&doc, secret).expect("application document is complete"),
    ))
}

#[openapi(tag = "OAuth")]
#[post("/applications/<client_id>/rotate-secret")]
pub async fn application_rotate_secret(
    db: &State<Database>,
    session: Session,
    client_id: String,
) -> OAuthResult<ApplicationResponse> {
    let secret = random_value(32);
    let doc = match db.inner() { Database::MongoDb(mongo) => mongo.col::<Document>("oauth_applications").find_one_and_update(doc! { "_id": &client_id, "owner_id": &session.user_id, "public": false, "revoked": false }, doc! { "$set": { "secret_hash": digest(&secret) } }).return_document(ReturnDocument::After).await.map_err(|_| error(Status::InternalServerError, "server_error", "OAuth storage unavailable"))?, Database::Reference(_) => None };
    Ok(Json(
        app_response(
            &doc.ok_or_else(|| error(Status::NotFound, "invalid_client", "Application not found"))?,
            Some(secret),
        )
        .unwrap(),
    ))
}

#[openapi(tag = "OAuth")]
#[post("/applications/<client_id>/revoke")]
pub async fn application_revoke(
    db: &State<Database>,
    session: Session,
    client_id: String,
) -> std::result::Result<Status, (Status, Json<OAuthError>)> {
    if let Database::MongoDb(mongo) = db.inner() {
        let result = mongo
            .col::<Document>("oauth_applications")
            .update_one(
                doc! { "_id": &client_id, "owner_id": &session.user_id },
                doc! { "$set": { "revoked": true } },
            )
            .await
            .map_err(|_| {
                error(
                    Status::InternalServerError,
                    "server_error",
                    "OAuth storage unavailable",
                )
            })?;
        if result.matched_count == 0 {
            return Err(error(
                Status::NotFound,
                "invalid_client",
                "Application not found",
            ));
        }
        mongo
            .col::<Document>("oauth_access_tokens")
            .update_many(
                doc! { "client_id": &client_id },
                doc! { "$set": { "revoked": true } },
            )
            .await
            .ok();
        mongo
            .col::<Document>("oauth_refresh_tokens")
            .update_many(
                doc! { "client_id": &client_id },
                doc! { "$set": { "revoked": true } },
            )
            .await
            .ok();
    }
    Ok(Status::NoContent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_s256_accepts_only_the_matching_verifier() {
        let verifier = "a-secure-random-verifier";
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert!(pkce_matches(verifier, &challenge));
        assert!(!pkce_matches("another-verifier", &challenge));
    }

    #[test]
    fn redirect_validation_rejects_open_redirects() {
        assert!(validate_redirect_uri(
            "https://obsidian.tails1154.com/auth/callback"
        ));
        assert!(validate_redirect_uri("http://localhost:3000/callback"));
        assert!(!validate_redirect_uri(
            "http://obsidian.tails1154.com/callback"
        ));
        assert!(!validate_redirect_uri("javascript:alert(1)"));
    }

    #[test]
    fn scopes_are_intersection_checked_against_application_allowlist() {
        let allowed = vec!["identify".to_owned(), "servers".to_owned()];
        assert_eq!(
            validate_scopes("identify servers", &allowed).unwrap().len(),
            2
        );
        assert!(validate_scopes("identify permissions", &allowed).is_none());
        assert!(validate_scopes("identify unknown", &allowed).is_none());
    }

    #[test]
    fn sensitive_values_are_digest_only_and_codes_expire_quickly() {
        let value = random_value(32);
        assert_ne!(value, digest(&value));
        assert_eq!(AUTHORIZATION_CODE_TTL, Duration::from_secs(300));
        assert!(constant_time_equal(&digest(&value), &digest(&value)));
        assert!(!constant_time_equal(&digest(&value), &digest("different")));
    }
}
