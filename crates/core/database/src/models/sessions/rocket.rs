use crate::{Database, Session};
use revolt_result::Error;
use rocket::{
    http::Status,
    request::{FromRequest, Outcome},
    Request,
};

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Session {
    type Error = Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        // Browser navigations cannot attach X-Session-Token. Login sets this
        // Secure, HttpOnly cookie; keep the header path for API clients and
        // prefer it when both credentials are present.
        let token = request
            .headers()
            .get("x-session-token")
            .next()
            .map(str::to_owned)
            .or_else(|| {
                request
                    .cookies()
                    .get("__Host-tailstalk_session")
                    .map(|cookie| cookie.value().to_owned())
            });

        if let Some(token) = token {
            if let Ok(session) = request
                .rocket()
                .state::<Database>()
                .expect("`Database`")
                .fetch_session_by_token(&token)
                .await
            {
                Outcome::Success(session)
            } else {
                Outcome::Error((Status::Unauthorized, create_error!(InvalidSession)))
            }
        } else {
            Outcome::Error((Status::Unauthorized, create_error!(MissingHeaders)))
        }
    }
}
