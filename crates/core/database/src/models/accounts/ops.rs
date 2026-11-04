use revolt_result::Result;

use crate::Account;
use iso8601_timestamp::Timestamp;

#[cfg(feature = "mongodb")]
mod mongodb;
mod reference;

#[async_trait]
pub trait AbstractAccounts: Sync + Send {
    /// Find account by id
    async fn fetch_account(&self, id: &str) -> Result<Account>;

    /// Find account by normalised email
    async fn fetch_account_by_normalised_email(
        &self,
        normalised_email: &str,
    ) -> Result<Option<Account>>;

    /// Find account with active pending email verification
    async fn fetch_account_with_email_verification(&self, token: &str) -> Result<Account>;

    /// Find account with active password reset
    async fn fetch_account_with_password_reset(&self, token: &str) -> Result<Account>;

    /// Find account with active deletion token
    async fn fetch_account_with_deletion_token(&self, token: &str) -> Result<Account>;

    /// Find accounts which are due to be deleted
    async fn fetch_accounts_due_for_deletion(&self) -> Result<Vec<Account>>;

    /// Remove registration IP data older than the configured retention period.
    async fn clear_expired_registration_ips(&self, before: Timestamp) -> Result<()>;

    // Save account
    async fn save_account(&self, account: &Account) -> Result<()>;
}
