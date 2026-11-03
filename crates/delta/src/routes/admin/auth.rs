use revolt_rocket_okapi::{
    gen::OpenApiGenerator,
    request::{OpenApiFromRequest, RequestHeaderInput},
    revolt_okapi::openapi3::{MediaType, Parameter, ParameterValue},
};
use revolt_database::{Database, MFATicket, User};
use rocket::{
    http::Status,
    request::{FromRequest, Outcome},
    Request,
};
use schemars::schema::SchemaObject;

pub(crate) struct AdminUser {
    pub(crate) user_id: String,
}

/// Access guard for the reports workflow. Privileged account sessions must
/// include the same valid MFA ticket as the rest of the admin API. The
/// dedicated report bot remains allowed for automated report tracking.
pub(crate) struct ReportAccess;

// TGR is the dedicated report-tracking bot. Keep report access narrower than
// general admin access and do not grant it to unrelated bot tokens.
const TGR_BOT_ID: &str = "01M5HVWFWXS9M3GE1MRWY5ZYSB";

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AdminUser {
    type Error = revolt_result::Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        match request.guard::<User>().await {
            Outcome::Success(user) if user.privileged => {
                match request.guard::<MFATicket>().await {
                    Outcome::Success(ticket)
                        if ticket.validated
                            && !ticket.is_expired()
                            && ticket.account_id == user.id =>
                    {
                        Outcome::Success(AdminUser { user_id: user.id })
                    }
                    Outcome::Success(_) => Outcome::Error((
                        Status::Forbidden,
                        revolt_result::create_error!(InvalidToken),
                    )),
                    Outcome::Error((status, error)) => Outcome::Error((status, error)),
                    Outcome::Forward(status) => Outcome::Forward(status),
                }
            }
            Outcome::Success(_) => Outcome::Error((
                Status::Forbidden,
                revolt_result::create_error!(NotPrivileged),
            )),
            Outcome::Error((status, error)) => Outcome::Error((status, error)),
            Outcome::Forward(status) => Outcome::Forward(status),
        }
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for ReportAccess {
    type Error = revolt_result::Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        if let Outcome::Success(user) = request.guard::<User>().await {
            if user.privileged {
                match request.guard::<MFATicket>().await {
                    Outcome::Success(ticket)
                        if ticket.validated
                            && !ticket.is_expired()
                            && ticket.account_id == user.id =>
                    {
                        return Outcome::Success(ReportAccess);
                    }
                    Outcome::Success(_) => {
                        return Outcome::Error((
                            Status::Forbidden,
                            revolt_result::create_error!(InvalidToken),
                        ));
                    }
                    Outcome::Error((status, error)) => return Outcome::Error((status, error)),
                    Outcome::Forward(status) => return Outcome::Forward(status),
                }
            }
        }

        let Some(token) = request.headers().get_one("x-bot-token") else {
            return Outcome::Error((Status::Unauthorized, revolt_result::create_error!(InvalidSession)));
        };
        let db = request.rocket().state::<Database>().expect("`Database`");
        if let Ok(bot) = db.fetch_bot_by_token(token).await {
            if bot.id == TGR_BOT_ID {
                return Outcome::Success(ReportAccess);
            }
            if let Ok(owner) = db.fetch_user(&bot.owner).await {
                if owner.privileged {
                    return Outcome::Success(ReportAccess);
                }
            }
        }

        Outcome::Error((Status::Unauthorized, revolt_result::create_error!(InvalidSession)))
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
            description: Some("Authenticated session for a privileged account".to_string()),
            deprecated: false,
            allow_empty_value: false,
            value: ParameterValue::Content { content },
            extensions: schemars::Map::new(),
        }))
    }
}

impl<'r> OpenApiFromRequest<'r> for ReportAccess {
    fn from_request_input(
        gen: &mut OpenApiGenerator,
        name: String,
        required: bool,
    ) -> revolt_rocket_okapi::Result<RequestHeaderInput> {
        AdminUser::from_request_input(gen, name, required)
    }
}
