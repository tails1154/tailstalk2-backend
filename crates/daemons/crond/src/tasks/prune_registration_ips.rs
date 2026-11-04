use iso8601_timestamp::{Duration, Timestamp};
use revolt_config::config;
use revolt_database::{Database, AMQP};
use revolt_result::Result;
use tokio::time::sleep;

/// Remove registration IP data after its configured retention period.
pub async fn task(db: Database, _: AMQP) -> Result<()> {
    loop {
        let retention = config().await.api.privacy.registration_ip_retention_seconds;
        let before = Timestamp::now_utc()
            .checked_sub(Duration::seconds(retention as i64))
            .unwrap_or(Timestamp::UNIX_EPOCH);
        db.clear_expired_registration_ips(before).await?;
        sleep(std::time::Duration::from_secs(60 * 60)).await;
    }
}
