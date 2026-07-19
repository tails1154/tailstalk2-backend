use base64::{engine::general_purpose, Engine as _};
use revolt_database::Database;
use revolt_result::Result;
use revolt_rocket_okapi::{
    gen::OpenApiGenerator,
    request::{OpenApiFromRequest, RequestHeaderInput},
    revolt_okapi::openapi3::{MediaType, Parameter, ParameterValue},
};
use rocket::{http::Status, request::{FromRequest, Outcome}, Request, State};
use rocket::serde::json::Json;
use schemars::schema::SchemaObject;
use serde::Serialize;
use std::sync::LazyLock;

static ADMIN_HASH: LazyLock<String> = LazyLock::new(|| {
    let content = include_str!("../../../../../admin.toml");
    for line in content.lines() {
        if let Some(value) = line.strip_prefix("password_hash = ") {
            let value: &str = value;
            return value.trim_matches('"').to_string();
        }
    }
    panic!("password_hash not found in admin.toml");
});

pub(crate) struct AdminUser;

fn verify_admin_auth(auth_header: Option<&str>) -> bool {
    let Some(header) = auth_header else {
        return false;
    };
    let Some(encoded) = header.strip_prefix("Basic ") else {
        return false;
    };
    let Ok(decoded) = general_purpose::STANDARD.decode(encoded.as_bytes()) else {
        return false;
    };
    let Ok(credentials) = String::from_utf8(decoded) else {
        return false;
    };

    if let Some((_user, password)) = credentials.split_once(':') {
        return argon2::verify_encoded(&ADMIN_HASH, password.as_bytes()).unwrap_or(false);
    }

    false
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AdminUser {
    type Error = revolt_result::Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let auth_header = request.headers().get_one("Authorization");

        if verify_admin_auth(auth_header) {
            Outcome::Success(AdminUser)
        } else {
            Outcome::Error((
                Status::Unauthorized,
                revolt_result::create_error!(InvalidSession),
            ))
        }
    }
}

impl<'r> OpenApiFromRequest<'r> for AdminUser {
    fn from_request_input(
        _gen: &mut OpenApiGenerator,
        _name: String,
        _required: bool,
    ) -> revolt_rocket_okapi::Result<RequestHeaderInput> {
        let mut content = schemars::Map::new();
        content.insert(
            "Authorization".to_string(),
            MediaType {
                schema: Some(SchemaObject {
                    string: Some(Box::default()),
                    ..Default::default()
                }),
                example: None,
                examples: None,
                encoding: schemars::Map::new(),
                extensions: schemars::Map::new(),
            },
        );

        Ok(RequestHeaderInput::Parameter(Parameter {
            name: "Authorization".to_string(),
            location: "header".to_string(),
            required: true,
            description: Some("Basic auth".to_string()),
            deprecated: false,
            allow_empty_value: false,
            value: ParameterValue::Content { content },
            extensions: schemars::Map::new(),
        }))
    }
}

/// Simple test endpoint - no auth required
#[openapi(tag = "Admin")]
#[get("/test")]
pub async fn _admin_test() -> Json<&'static str> {
    Json("admin test ok")
}

#[derive(Serialize, JsonSchema, Debug)]
pub struct AdminStats {
    pub authenticated: bool,
    pub server_time: String,
}

/// # Admin Stats
///
/// Fetch admin dashboard stats. Requires HTTP Basic Auth.
#[openapi(tag = "Admin")]
#[get("/")]
pub async fn admin_stats(
    _admin: AdminUser,
    _db: &State<Database>,
) -> Result<Json<AdminStats>> {
    Ok(Json(AdminStats {
        authenticated: true,
        server_time: chrono::Utc::now().to_rfc3339(),
    }))
}
