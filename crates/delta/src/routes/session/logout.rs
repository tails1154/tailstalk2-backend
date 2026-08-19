//! Logout of current session
//! POST /session/logout
use revolt_database::{Database, Session};
use revolt_result::Result;
use rocket::{
    http::{Cookie, CookieJar},
    State,
};
use rocket_empty::EmptyResponse;

/// # Logout
///
/// Delete current session.
#[openapi(tag = "Session")]
#[post("/logout")]
pub async fn logout(
    db: &State<Database>,
    cookies: &CookieJar<'_>,
    session: Session,
) -> Result<EmptyResponse> {
    session.delete(db).await.map(|_| {
        cookies.remove(Cookie::build("__Host-tailstalk_session").path("/").build());
        EmptyResponse
    })
}

#[cfg(test)]
mod tests {
    use crate::{rocket, util::test::TestHarness};
    use revolt_database::events::client::EventV1;
    use revolt_result::ErrorType;
    use rocket::http::{Header, Status};

    #[rocket::async_test]
    async fn success() {
        let mut harness = TestHarness::new().await;
        let (_, session, _) = harness.new_user().await;

        let res = harness
            .client
            .post("/auth/session/logout")
            .header(Header::new("X-Session-Token", session.token))
            .dispatch()
            .await;

        assert_eq!(res.status(), Status::NoContent);
        drop(res);
        assert!(matches!(
            harness
                .db
                .fetch_session(&session.id)
                .await
                .unwrap_err()
                .error_type,
            ErrorType::UnknownUser
        ));

        let event = harness
            .wait_for_event(&format!("{}!", &session.user_id), |evt| {
                matches!(evt, EventV1::DeleteSession { .. })
            })
            .await;
        if let EventV1::DeleteSession {
            user_id,
            session_id,
        } = event
        {
            assert_eq!(user_id, session.user_id);
            assert_eq!(session_id, session.id);
        } else {
            panic!("Received incorrect event type. {:?}", event);
        }
    }

    #[rocket::async_test]
    async fn fail_invalid_session() {
        let harness = TestHarness::new().await;

        let res = harness
            .client
            .post("/auth/session/logout")
            .header(Header::new("X-Session-Token", "invalid"))
            .dispatch()
            .await;

        assert_eq!(res.status(), Status::Unauthorized);
    }

    #[rocket::async_test]
    async fn fail_no_session() {
        let harness = TestHarness::new().await;

        let res = harness.client.post("/auth/session/logout").dispatch().await;

        assert_eq!(res.status(), Status::Unauthorized);
    }
}
