use std::collections::{BTreeMap, HashMap};
use std::convert::TryInto;

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use revolt_config::config;
use revolt_database::{
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, PartialServer, Role, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ServerBackup {
    pub version: u8,
    pub server_id: String,
    pub name: String,
    pub description: Option<String>,
    #[schemars(skip)]
    pub roles: HashMap<String, Role>,
    #[schemars(skip)]
    pub categories: Option<Vec<revolt_database::Category>>,
    pub default_permissions: i64,
    /// Base64 Ed25519 signature over every other field
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// The exact fields covered by the signature, in a fixed order with sorted roles
#[derive(Serialize)]
struct SignedPayload<'a> {
    version: u8,
    server_id: &'a String,
    name: &'a String,
    description: &'a Option<String>,
    roles: BTreeMap<&'a String, &'a Role>,
    categories: &'a Option<Vec<revolt_database::Category>>,
    default_permissions: i64,
}

fn signed_payload(backup: &ServerBackup) -> Result<Vec<u8>> {
    let payload = SignedPayload {
        version: backup.version,
        server_id: &backup.server_id,
        name: &backup.name,
        description: &backup.description,
        roles: backup.roles.iter().collect(),
        categories: &backup.categories,
        default_permissions: backup.default_permissions,
    };
    serde_json::to_vec(&payload).map_err(|_| create_error!(InternalError))
}

/// Load the instance's backup signing key
async fn backup_signing_key() -> Result<SigningKey> {
    let encoded = config().await.api.backup.signing_key;
    let disabled = || {
        create_error!(FeatureDisabled {
            feature: "server_backup_signing".to_string()
        })
    };
    if encoded.is_empty() {
        return Err(disabled());
    }
    let bytes = STANDARD.decode(encoded).map_err(|_| disabled())?;
    let seed: [u8; 32] = bytes.try_into().map_err(|_| disabled())?;
    Ok(SigningKey::from_bytes(&seed))
}

fn sign_backup(key: &SigningKey, backup: &ServerBackup) -> Result<String> {
    let signature = key.sign(&signed_payload(backup)?);
    Ok(STANDARD.encode(signature.to_bytes()))
}

fn verify_backup(key: &SigningKey, backup: &ServerBackup) -> Result<()> {
    let encoded = backup
        .signature
        .as_deref()
        .ok_or_else(|| create_error!(InvalidProperty))?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| create_error!(InvalidProperty))?;
    let signature = Signature::from_slice(&bytes).map_err(|_| create_error!(InvalidProperty))?;
    key.verifying_key()
        .verify(&signed_payload(backup)?, &signature)
        .map_err(|_| create_error!(InvalidProperty))
}

fn validate_backup(backup: &ServerBackup, server_id: &str) -> Result<()> {
    if backup.version != 1 || backup.server_id != server_id || backup.name.trim().is_empty() {
        return Err(create_error!(InvalidProperty));
    }
    Ok(())
}

#[openapi(tag = "Server Information")]
#[get("/<target>/backup")]
pub async fn export(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerBackup>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;
    let key = backup_signing_key().await?;
    let mut backup = ServerBackup {
        version: 1,
        server_id: server.id,
        name: server.name,
        description: server.description,
        roles: server.roles,
        categories: server.categories,
        default_permissions: server.default_permissions,
        signature: None,
    };
    backup.signature = Some(sign_backup(&key, &backup)?);
    Ok(Json(backup))
}

#[openapi(tag = "Server Information")]
#[post("/<target>/backup", data = "<data>")]
pub async fn restore(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<ServerBackup>,
) -> Result<Json<ServerBackup>> {
    let mut server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;
    let backup = data.into_inner();
    validate_backup(&backup, &server.id)?;
    verify_backup(&backup_signing_key().await?, &backup)?;
    let partial = PartialServer {
        name: Some(backup.name.clone()),
        description: backup.description.clone(),
        roles: Some(backup.roles.clone()),
        categories: backup.categories.clone(),
        default_permissions: Some(backup.default_permissions),
        ..Default::default()
    };
    server.update(db, partial, vec![]).await?;
    Ok(Json(backup))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::test::TestHarness;
    use revolt_models::v0::DataCreateServer;
    use rocket::http::{ContentType, Header, Status};

    #[test]
    fn rejects_wrong_version_server_and_empty_name() {
        let backup = ServerBackup {
            version: 2,
            server_id: "server".into(),
            name: "Name".into(),
            description: None,
            roles: HashMap::new(),
            categories: None,
            default_permissions: 0,
            signature: None,
        };
        assert!(validate_backup(&backup, "server").is_err());

        let mut backup = backup;
        backup.version = 1;
        backup.server_id = "other".into();
        assert!(validate_backup(&backup, "server").is_err());

        backup.server_id = "server".into();
        backup.name = "   ".into();
        assert!(validate_backup(&backup, "server").is_err());
    }

    fn test_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn signed_backup(key: &SigningKey) -> ServerBackup {
        let mut roles = HashMap::new();
        roles.insert(
            "role".to_string(),
            Role {
                id: "role".into(),
                name: "Admin".into(),
                permissions: Default::default(),
                colour: None,
                hoist: false,
                rank: 0,
                icon: None,
            },
        );
        let mut backup = ServerBackup {
            version: 1,
            server_id: "server".into(),
            name: "Name".into(),
            description: Some("Desc".into()),
            roles,
            categories: None,
            default_permissions: 0,
            signature: None,
        };
        backup.signature = Some(sign_backup(key, &backup).unwrap());
        backup
    }

    #[test]
    fn accepts_untouched_signed_backup() {
        let key = test_key();
        assert!(verify_backup(&key, &signed_backup(&key)).is_ok());
    }

    #[test]
    fn rejects_missing_or_tampered_signature() {
        let key = test_key();

        let mut unsigned = signed_backup(&key);
        unsigned.signature = None;
        assert!(verify_backup(&key, &unsigned).is_err());

        let mut other_server = signed_backup(&key);
        other_server.server_id = "other".into();
        assert!(verify_backup(&key, &other_server).is_err());

        let mut escalated = signed_backup(&key);
        escalated.roles.get_mut("role").unwrap().rank = -1;
        escalated.default_permissions = i64::MAX;
        assert!(verify_backup(&key, &escalated).is_err());

        let other_key = SigningKey::from_bytes(&[9u8; 32]);
        assert!(verify_backup(&other_key, &signed_backup(&key)).is_err());
    }

    #[ignore = "requires the MongoDB test service"]
    #[rocket::async_test]
    async fn export_then_restore_round_trip() {
        let harness = TestHarness::new().await;
        let (_, session, user) = harness.new_user().await;
        let (server, _) = revolt_database::Server::create(
            &harness.db,
            DataCreateServer {
                name: TestHarness::rand_string(),
                ..Default::default()
            },
            &user,
            false,
        )
        .await
        .unwrap();

        let token = Header::new("x-session-token", session.token.to_string());
        let response = harness
            .client
            .get(format!("/servers/{}/backup", server.id))
            .header(token.clone())
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Ok);
        let backup = response.into_json::<ServerBackup>().await.unwrap();

        let response = harness
            .client
            .post(format!("/servers/{}/backup", server.id))
            .header(ContentType::JSON)
            .header(token)
            .body(serde_json::to_string(&backup).unwrap())
            .dispatch()
            .await;
        assert_eq!(response.status(), Status::Ok);
    }
}
