//! AI safety review of new messages from school accounts.
//!
//! Scope: only accounts whose verified email ends in `@cats.angletonisd.net`. Every other message
//! is skipped before any AI call. Flags go to the staff review queue. The scanner never warns,
//! suspends, or bans anyone itself, and nothing is emailed.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use revolt_config::{config, ApiAisd};
use revolt_database::mongodb::bson::{self, doc};
use futures::StreamExt;
use revolt_database::{Database, EmailVerification, Message};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Semaphore;

const SCHOOL_DOMAIN: &str = "@cats.angletonisd.net";

const SYSTEM_PROMPT: &str = "You review chat messages for a school platform. Decide whether the message \
expresses a real intention to harm oneself (self_harm) or a real threat of violence against another person (threat), or intimidation, insults or abuse aimed at a specific person (harassment). \
Harassment includes threats of consequences that are not violence, such as \"you're going down\", \"you'll regret this\", or threats to shut down, ruin, or take over a person's account, server or work. \
When a message is aimed at a named person with that kind of pressure, it is harassment even if no violence is mentioned. \
Quoted text, fiction, jokes, song lyrics, or discussion of a topic without intent do not count. \
Earlier messages from the channel are given for context only. Judge only the message marked as the message to judge, \
and use the earlier messages to work out what words like \"it\", \"this\" or \"do it\" refer to. \
Sarcasm and exaggeration are not intent, and a joke marker means it is not a real threat: words like \"trust\", \"lol\", \"jk\", \"/s\" or \"haha\" at the end usually mean sarcasm. \
For example \"im gonna execute you trust\" is sarcasm about a company or a product, not a threat, so do not flag it. \
A message that continues a plan stated earlier counts as intent. \
Short strings are often not self-harm. \"sh\" alone means nothing: it can be shh, a typo for ssh, or the shell sh. \
Only flag \"sh\" when the message says the sender plans to self-harm, for example \"gonna do sh\" or \"gonna do sh to myself\". \
A stated plan still counts if the sender adds that it is a test or that they are not really considering it: flag it and let staff decide. \
Reply with only a JSON object: {\"self_harm\": true or false, \"threat\": true or false, \"harassment\": true or false, \"confidence\": a number from 0 to 1, \"reason\": one short sentence saying why, quoting the key words}.";

#[derive(Debug, Deserialize)]
struct Verdict {
    #[serde(default)]
    self_harm: bool,
    #[serde(default)]
    threat: bool,
    #[serde(default)]
    harassment: bool,
    #[serde(default)]
    confidence: f32,
    /// One sentence from the model saying why it decided this
    #[serde(default)]
    reason: String,
}

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        // No timeout: the classifier waits for llama-server to finish, however long that takes
        reqwest::Client::builder()
            .build()
            .expect("valid reqwest client")
    })
}

/// Limits how many llama-server requests run at once
fn limit() -> &'static Semaphore {
    static LIMIT: OnceLock<Semaphore> = OnceLock::new();
    // llama-server runs one request at a time (-np 1), so queue here instead of at the server
    LIMIT.get_or_init(|| Semaphore::new(1))
}

/// Whether a verified account belongs to the school scope
pub fn in_scope(email: &str) -> bool {
    email.to_lowercase().ends_with(SCHOOL_DOMAIN)
}

/// Whether an email may open the staff panel. Only angletonisd.net addresses and the configured testing addresses.
pub fn panel_access_allowed(email: &str, aisd: &ApiAisd) -> bool {
    let email = email.trim().to_lowercase();
    let Some((_, domain)) = email.rsplit_once('@') else {
        return false;
    };
    aisd.panel_allowed_domains.iter().any(|d| d.to_lowercase() == domain)
        || aisd.panel_allowed_emails.iter().any(|e| e.to_lowercase() == email)
}

/// Removes llama-server thinking blocks so only the final answer is parsed
fn strip_thinking(text: &str) -> String {
    let mut out = text.to_string();
    while let (Some(start), Some(end)) = (out.find("<think>"), out.find("</think>")) {
        if end < start {
            break;
        }
        out.replace_range(start..end + "</think>".len(), "");
    }
    out
}

/// Pulls the JSON object out of the model's reply, even if it adds prose or code fences
fn parse_verdict(text: &str) -> Result<Verdict, String> {
    let text = strip_thinking(text);
    let start = text.find('{').ok_or("no JSON object in reply")?;
    let end = text.rfind('}').ok_or("no JSON object in reply")?;
    serde_json::from_str(&text[start..=end]).map_err(|e| e.to_string())
}

/// Room for the model to think before it must answer. Plenty for hard messages, not enough to loop for long.
const FIRST_MAX_TOKENS: u32 = 2048;
/// Smaller budget for the single retry after the model ran out of room
const RETRY_MAX_TOKENS: u32 = 1024;
/// How much of the cut-off reasoning the retry is shown, roughly 500 tokens
const RETRY_TAIL_CHARS: usize = 2000;

struct Reply {
    /// The final answer, or None if the model was cut off before it answered
    content: Option<String>,
    /// The model's reasoning trace, when llama-server includes one
    reasoning: String,
}

async fn ask(aisd: &ApiAisd, messages: Vec<serde_json::Value>, max_tokens: u32) -> Result<Reply, String> {
    let url = format!("{}/v1/chat/completions", aisd.url.trim_end_matches('/'));
    let body = json!({
        "model": aisd.model,
        "temperature": 0,
        "max_tokens": max_tokens,
        "messages": messages,
    });

    let mut request = http().post(url).json(&body);
    if !aisd.api_key.is_empty() {
        request = request.bearer_auth(&aisd.api_key);
    }

    let response: serde_json::Value = request
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let message = &response["choices"][0]["message"];
    let content = message["content"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let reasoning = message["reasoning_content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok(Reply { content, reasoning })
}

/// Returns the verdict and the model's own reasoning trace, when llama-server includes one.
/// If the model runs out of room while still thinking, it is retried once with the end of that
/// reasoning and a request to reason less. A second failure is an error, which becomes a staff flag.
async fn classify(aisd: &ApiAisd, content: &str) -> Result<(Verdict, String), String> {
    let system = json!({ "role": "system", "content": SYSTEM_PROMPT });
    let user = json!({ "role": "user", "content": content });

    let first = ask(aisd, vec![system.clone(), user.clone()], FIRST_MAX_TOKENS).await?;
    if let Some(text) = first.content {
        return Ok((parse_verdict(&text)?, first.reasoning));
    }

    let tail: String = {
        let mut chars: Vec<char> = first.reasoning.chars().rev().take(RETRY_TAIL_CHARS).collect();
        chars.reverse();
        chars.into_iter().collect()
    };
    let note = format!(
        "Your last response hit the reasoning limit. Here is the end of your reasoning:\n{tail}\n\nReason less and reply with only the JSON object."
    );
    let retry = ask(
        aisd,
        vec![system, user, json!({ "role": "user", "content": note })],
        RETRY_MAX_TOKENS,
    )
    .await?;
    match retry.content {
        Some(text) => Ok((parse_verdict(&text)?, retry.reasoning)),
        None => Err("reasoning limit hit on the retry too".to_string()),
    }
}

/// The last few messages in the channel before this one, oldest first, so the model can read what "it" refers to
async fn earlier_messages(db: &Database, message: &Message, author_id: &str) -> String {
    let Database::MongoDb(mongo) = db else {
        return String::new();
    };
    let Ok(mut cursor) = mongo
        .col::<bson::Document>("messages")
        .find(doc! { "channel": &message.channel, "_id": { "$lt": &message.id } })
        .sort(doc! { "_id": -1 })
        .limit(5)
        .await
    else {
        return String::new();
    };
    let mut lines = Vec::new();
    while let Some(Ok(d)) = cursor.next().await {
        let content = d.get_str("content").unwrap_or_default();
        if content.trim().is_empty() {
            continue;
        }
        let who = if d.get_str("author").unwrap_or_default() == author_id {
            "sender"
        } else {
            "other person"
        };
        let text: String = content.chars().take(300).collect();
        lines.push(format!("{who}: {text}"));
    }
    lines.reverse();
    lines.join("\n")
}

/// Messages queued in the current batch, and how many of them have finished. Counts only, never text.
static BATCH_TOTAL: AtomicUsize = AtomicUsize::new(0);
static BATCH_DONE: AtomicUsize = AtomicUsize::new(0);

/// (finished, total) for the current batch. Total is 0 when the scanner is idle.
pub fn scan_progress() -> (usize, usize) {
    (BATCH_DONE.load(Ordering::Relaxed), BATCH_TOTAL.load(Ordering::Relaxed))
}

/// Starts a new batch when work arrives, and closes it when every queued message has finished
fn finish_one() {
    let done = BATCH_DONE.fetch_add(1, Ordering::Relaxed) + 1;
    if done >= BATCH_TOTAL.load(Ordering::Relaxed) {
        BATCH_TOTAL.store(0, Ordering::Relaxed);
        BATCH_DONE.store(0, Ordering::Relaxed);
    }
}

/// Phrases that mean a self-harm plan or crisis on their own. Matching is case-insensitive.
const SELF_HARM_PHRASES: &[&str] = &[
    "kill myself",
    "end my life",
    "want to die",
    "wanna die",
    "dont need to be here",
    "don't need to be here",
    "goodbye world",
    "gonna end it",
    "going to end it",
    "end it all",
    "suicide",
];

/// Phrases that mean a threat of serious violence on their own. Matching is case-insensitive.
/// Kept to explicit intent, so discussing a topic such as history does not match.
const VIOLENCE_PHRASES: &[&str] = &[
    "commit genocide",
    "commit a genocide",
    "shoot up the school",
    "bomb the school",
    "gonna kill you",
    "going to kill you",
    "i will kill you",
    "i'll kill you",
];

/// Lowercases text, keeps only letters, and collapses runs of the same letter, so
/// "k i l llll you" and "kill you" compare equal. Applied to messages and phrases alike.
fn normalize(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_lowercase()) {
        if out.chars().last() != Some(c) {
            out.push(c);
        }
    }
    out
}

/// The kind and phrase of the first rule that matches, if any. Crisis phrases are checked first.
/// Matching ignores spacing, punctuation, capitals and repeated letters.
fn rule_match(content: &str) -> Option<(&'static str, &'static str)> {
    let haystack = normalize(content);
    let hit = |phrase: &&'static str| haystack.contains(&normalize(phrase));
    if let Some(phrase) = SELF_HARM_PHRASES.iter().copied().find(hit) {
        return Some(("self_harm", phrase));
    }
    VIOLENCE_PHRASES.iter().copied().find(hit).map(|phrase| ("threat", phrase))
}

/// Runs the review in the background so sending a message is never delayed or blocked by it
pub fn spawn_check(db: Database, message: Message, author_id: String) {
    tokio::spawn(async move {
        // Only count work when scanning is switched on
        if !config().await.api.aisd.enabled {
            return;
        }
        BATCH_TOTAL.fetch_add(1, Ordering::Relaxed);
        if let Err(err) = check(&db, &message, &author_id).await {
            log::error!("Safety review failed for message {}: {err}", message.id);
        }
        finish_one();
    });
}

async fn check(db: &Database, message: &Message, author_id: &str) -> Result<(), String> {
    let settings = config().await.api.aisd;
    if !settings.enabled {
        return Ok(());
    }

    let account = db.fetch_account(author_id).await.map_err(|e| format!("{e:?}"))?;
    if !matches!(account.verification, EmailVerification::Verified) || !in_scope(&account.email) {
        return Ok(());
    }

    let Some(content) = message.content.as_deref().filter(|c| !c.trim().is_empty()) else {
        return Ok(());
    };

    // Explicit crisis and violence phrases are flagged directly, so a model miss can't hide them
    if let Some((kind, phrase)) = rule_match(content) {
        let reason = format!("Matched a {kind} phrase (\"{phrase}\") by rule, not by the model.");
        log_decision(
            message,
            author_id,
            "rule",
            content,
            json!({ "flagged": true, "kind": kind, "confidence": 1.0, "reason": reason }),
            "",
        );
        store_flag(db, message, author_id, kind, 1.0, &reason, "").await;
        return Ok(());
    }

    let _permit = limit().acquire().await.map_err(|e| e.to_string())?;
    let earlier = earlier_messages(db, message, author_id).await;
    let judged = if earlier.is_empty() {
        content.to_string()
    } else {
        format!("Earlier messages in this channel:\n{earlier}\n\nMessage to judge (from the sender):\n{content}")
    };
    let (verdict, reasoning) = match classify(&settings, &judged).await {
        Ok(result) => result,
        Err(err) => {
            // Staff get a flag, so a message is never skipped silently
            log_decision(
                message,
                author_id,
                "error",
                &judged,
                json!({ "flagged": true, "kind": "review failed", "error": err }),
                "",
            );
            store_flag(
                db,
                message,
                author_id,
                "review failed",
                0.0,
                "The classifier could not review this message. Please review it by hand.",
                "",
            )
            .await;
            return Err(err);
        }
    };

    let mut kinds = Vec::new();
    if verdict.self_harm {
        kinds.push("self_harm");
    }
    if verdict.threat {
        kinds.push("threat");
    }
    if verdict.harassment {
        kinds.push("harassment");
    }
    let kind = kinds.join(", ");
    let flagged = !kinds.is_empty() && verdict.confidence >= settings.threshold;
    log_decision(
        message,
        author_id,
        "model",
        &judged,
        json!({
            "flagged": flagged,
            "kind": kind,
            "self_harm": verdict.self_harm,
            "threat": verdict.threat,
            "harassment": verdict.harassment,
            "confidence": verdict.confidence,
            "reason": verdict.reason,
        }),
        &reasoning,
    );
    if !flagged {
        return Ok(());
    }

    // Flags are shown as notifications in the AISD panel. Nothing is emailed.
    store_flag(db, message, author_id, &kind, verdict.confidence, &verdict.reason, &reasoning).await;
    Ok(())
}

/// Where every classifier decision is written, one JSON object per line. Override with AISD_DECISION_LOG.
fn decision_log_path() -> String {
    std::env::var("AISD_DECISION_LOG").unwrap_or_else(|_| "/logs/aisd-decisions.jsonl".to_string())
}

/// Appends one decision to the decision log. The file holds the judged message text, so keep it private.
/// A write failure is logged and never stops the review.
fn log_decision(
    message: &Message,
    author_id: &str,
    source: &str,
    judged: &str,
    outcome: serde_json::Value,
    reasoning: &str,
) {
    static WRITE: Mutex<()> = Mutex::new(());
    let line = json!({
        "ts": iso8601_timestamp::Timestamp::now_utc().to_string(),
        "message_id": message.id,
        "channel_id": message.channel,
        "author_id": author_id,
        "source": source,
        "outcome": outcome,
        "reasoning": reasoning.chars().take(8000).collect::<String>(),
        "judged": judged,
    })
    .to_string();
    // One line per write, so concurrent reviews never interleave
    let _guard = WRITE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(decision_log_path())
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(err) = written {
        log::error!("Failed to write the safety decision log for message {}: {err}", message.id);
    }
}

async fn store_flag(
    db: &Database,
    message: &Message,
    author_id: &str,
    kind: &str,
    confidence: f32,
    reason: &str,
    reasoning: &str,
) {
    let Database::MongoDb(mongo) = db else {
        return;
    };
    let result = mongo
        .col::<bson::Document>("safety_flags")
        .insert_one(doc! {
            "kind": kind,
            "confidence": confidence as f64,
            "reason": reason,
            "reasoning": reasoning.chars().take(8000).collect::<String>(),
            "actions": Vec::<bson::Bson>::new(),
            "message_id": &message.id,
            "channel_id": &message.channel,
            "author_id": author_id,
            "created_at": iso8601_timestamp::Timestamp::now_utc().to_string(),
            "reviewed": false,
        })
        .await;
    if let Err(err) = result {
        log::error!("Failed to store safety flag for message {}: {err:?}", message.id);
    }
}

#[cfg(test)]
mod tests {
    use super::rule_match;

    #[test]
    fn rules_ignore_spacing_case_and_repeated_letters() {
        assert_eq!(rule_match("I'm gonnaaa KILL you").map(|m| m.0), Some("threat"));
        assert_eq!(rule_match("s u i iii c i d e").map(|m| m.0), Some("self_harm"));
        assert_eq!(rule_match("i want to diiiie").map(|m| m.0), Some("self_harm"));
        assert_eq!(rule_match("im gonna commit    genocide").map(|m| m.0), Some("threat"));
    }

    #[test]
    fn rules_do_not_match_ordinary_text() {
        assert_eq!(rule_match("im gonna execute you trust"), None);
        assert_eq!(rule_match("shh, the test is at 10"), None);
        assert_eq!(rule_match("we studied genocide in history class"), None);
    }
}
