use revolt_database::{
    mongodb::bson::{doc, Document},
    util::{permissions::DatabasePermissionQuery, reference::Reference},
    Database, PartialMember, User,
};
use revolt_permissions::{calculate_server_permissions, ChannelPermission};
use revolt_result::{create_database_error, Result};
use rocket::{serde::json::Json, State};
use schemars::JsonSchema;
use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ServerOnboarding {
    pub enabled: bool,
    pub title: String,
    pub message: String,
    pub rules: String,
    #[serde(default)]
    pub questions: Vec<OnboardingQuestion>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OnboardingQuestion {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub multiple: bool,
    pub options: Vec<OnboardingOption>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct OnboardingOption {
    pub id: String,
    pub label: String,
    pub role_id: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
pub struct CompleteOnboarding {
    #[serde(default)]
    pub answers: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct CompleteOnboardingResponse {
    pub roles: Vec<String>,
}

fn validate_questions(
    questions: &[OnboardingQuestion],
    server: &revolt_database::Server,
) -> Result<()> {
    let mut question_ids = HashSet::new();
    for question in questions {
        if question.id.trim().is_empty() || !question_ids.insert(&question.id) {
            return Err(revolt_result::create_error!(InvalidProperty));
        }

        let mut option_ids = HashSet::new();
        for option in &question.options {
            if option.id.trim().is_empty()
                || option.label.trim().is_empty()
                || !option_ids.insert(&option.id)
                || !server.roles.contains_key(&option.role_id)
                || option.role_id == server.id
            {
                return Err(revolt_result::create_error!(InvalidProperty));
            }
        }
    }
    Ok(())
}

/// Fetch onboarding settings for a server member.
#[openapi(tag = "Server Information")]
#[get("/<target>/onboarding")]
pub async fn fetch(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
) -> Result<Json<ServerOnboarding>> {
    let server = target.as_server(db).await?;
    db.fetch_member(&server.id, &user.id).await?;

    let settings = match &**db {
        Database::MongoDb(mongo) => mongo
            .col::<ServerOnboarding>("server_onboarding")
            .find_one(doc! { "_id": &server.id })
            .await
            .map_err(|_| create_database_error!("find_one", "server_onboarding"))?
            .unwrap_or_else(|| ServerOnboarding {
                enabled: false,
                title: format!("Welcome to {}", server.name),
                ..Default::default()
            }),
        _ => ServerOnboarding {
            enabled: false,
            title: format!("Welcome to {}", server.name),
            ..Default::default()
        },
    };

    Ok(Json(settings))
}

/// Update onboarding settings. Requires ManageServer.
#[openapi(tag = "Server Information")]
#[patch("/<target>/onboarding", data = "<data>")]
pub async fn update(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<ServerOnboarding>,
) -> Result<Json<ServerOnboarding>> {
    let server = target.as_server(db).await?;
    let mut query = DatabasePermissionQuery::new(db, &user).server(&server);
    calculate_server_permissions(&mut query)
        .await
        .throw_if_lacking_channel_permission(ChannelPermission::ManageServer)?;

    let settings = data.into_inner();
    validate_questions(&settings.questions, &server)?;
    match &**db {
        Database::MongoDb(mongo) => {
            let mut document = revolt_database::mongodb::bson::to_document(&settings)
                .map_err(|_| create_database_error!("serialize", "server_onboarding"))?;
            document.insert("_id", &server.id);
            mongo
                .col::<Document>("server_onboarding")
                .replace_one(doc! { "_id": &server.id }, document)
                .upsert(true)
                .await
                .map_err(|_| create_database_error!("replace_one", "server_onboarding"))?;
        }
        _ => return Err(create_database_error!("replace_one", "server_onboarding")),
    }

    Ok(Json(settings))
}

/// Apply the roles selected in onboarding.
#[openapi(tag = "Server Information")]
#[post("/<target>/onboarding/complete", data = "<data>")]
pub async fn complete(
    db: &State<Database>,
    user: User,
    target: Reference<'_>,
    data: Json<CompleteOnboarding>,
) -> Result<Json<CompleteOnboardingResponse>> {
    let server = target.as_server(db).await?;
    let mut member = db.fetch_member(&server.id, &user.id).await?;
    let settings = match &**db {
        Database::MongoDb(mongo) => mongo
            .col::<ServerOnboarding>("server_onboarding")
            .find_one(doc! { "_id": &server.id })
            .await
            .map_err(|_| create_database_error!("find_one", "server_onboarding"))?
            .unwrap_or_default(),
        _ => ServerOnboarding::default(),
    };

    validate_questions(&settings.questions, &server)?;

    let mut selected_roles = HashSet::new();
    for (question_id, option_ids) in data.into_inner().answers {
        let question = settings
            .questions
            .iter()
            .find(|question| question.id == question_id)
            .ok_or_else(|| revolt_result::create_error!(InvalidProperty))?;
        if !question.multiple && option_ids.len() > 1 {
            return Err(revolt_result::create_error!(InvalidProperty));
        }
        for option_id in option_ids {
            let option = question
                .options
                .iter()
                .find(|option| option.id == option_id)
                .ok_or_else(|| revolt_result::create_error!(InvalidProperty))?;
            selected_roles.insert(option.role_id.clone());
        }
    }

    let mut roles = member.roles.clone();
    roles.extend(selected_roles.iter().cloned());
    roles.sort();
    roles.dedup();
    member
        .update(
            db,
            PartialMember {
                roles: Some(roles),
                ..Default::default()
            },
            vec![],
        )
        .await?;

    Ok(Json(CompleteOnboardingResponse {
        roles: selected_roles.into_iter().collect(),
    }))
}
