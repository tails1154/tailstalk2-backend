use std::time::Duration;

use revolt_database::Database;
use revolt_result::{ErrorType, Result};
use tokio::time::sleep;

pub async fn task(db: Database, _: revolt_database::AMQP) -> Result<()> {
    loop {
        let accounts = db.fetch_accounts_due_for_deletion().await?;
        let count = accounts.len();

        for mut account in accounts {
            match db.fetch_user(&account.id).await {
                Ok(mut user) => user.delete(&db).await?,
                Err(error) if matches!(&error.error_type, ErrorType::NotFound) => {
                    // A previous deletion attempt may have removed the user
                    // before the account tombstone was written. Continue the
                    // account cleanup rather than blocking every later job.
                    log::warn!("User {} is already absent during deletion", account.id);
                }
                Err(error) => return Err(error),
            }
            account.mark_deleted(&db).await?;
        }

        log::info!("Deleted {count} accounts.");

        sleep(Duration::from_hours(1)).await
    }
}
