#!/usr/bin/env bash
set -Eeuo pipefail

# Create a redacted data-access export for a verified requester.
# This is intentionally operator-run; never expose the output directory over
# HTTP without verifying the requester's identity first.

umask 077

if (( $# != 2 )); then
  echo "Usage: $0 USER_ID OUTPUT_DIRECTORY" >&2
  exit 2
fi

user_id="$1"
output_dir="$2"

if [[ ! "$user_id" =~ ^[0-9A-HJKMNP-TV-Z]{26}$ ]]; then
  echo "USER_ID must be a valid TailsTalk 2 ULID" >&2
  exit 2
fi
if [[ "$output_dir" != /* || "$output_dir" == /home/tails1154/stoat/data* ]]; then
  echo "OUTPUT_DIRECTORY must be an absolute path outside the live data directory" >&2
  exit 2
fi

command -v docker >/dev/null
install -d -m 0700 "$output_dir"
staging="$(mktemp -d "$output_dir/.tailstalk2-export.XXXXXX")"
container="${EXPORT_DATABASE_CONTAINER:-stoat-database-1}"
database="${EXPORT_DATABASE:-revolt}"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
archive="$output_dir/tailstalk2-user-$user_id-$timestamp.tar.gz"

cleanup() { rm -rf "$staging"; }
trap cleanup EXIT

export_collection() {
  local collection="$1"
  local query="$2"
  local fields="$3"

  docker exec "$container" mongoexport \
    --db "$database" \
    --collection "$collection" \
    --query "$query" \
    --fields "$fields" \
    --jsonArray \
    > "$staging/$collection.json"
}

printf '%s\n' \
  "created_at=$timestamp" \
  "user_id=$user_id" \
  "redaction=password hashes, MFA secrets, session tokens, reset tokens, webhook tokens" \
  "note=uploaded binary content requires a separate verified operator handoff" \
  > "$staging/README.txt"

export_collection accounts \
  "{\"_id\":\"$user_id\"}" \
  "_id,email,email_normalised,verification.status,deletion.status,subscription.provider,subscription.tier,subscription.active_until,cash_payment_request.status"
export_collection users \
  "{\"_id\":\"$user_id\"}" \
  "_id,username,discriminator,display_name,pronouns,avatar,status,profile_content,badges,flags"
export_collection sessions \
  "{\"user_id\":\"$user_id\"}" \
  "_id,user_id,name,last_seen,origin"
export_collection bots \
  "{\"owner\":\"$user_id\"}" \
  "_id,owner,public,analytics,discoverable,interactions_url,terms_of_service_url,privacy_policy_url,flags"
export_collection oauth_applications \
  "{\"owner_id\":\"$user_id\"}" \
  "_id,name,owner_id,redirect_uris,allowed_scopes,public,revoked,created_at"
export_collection servers \
  "{\"owner\":\"$user_id\"}" \
  "_id,owner,name,description,channels,categories,system_messages,roles,default_permissions,icon,banner,flags,nsfw,analytics,discoverable,admin_suspended"
export_collection channels \
  "{\"\$or\":[{\"user\":\"$user_id\"},{\"recipients\":\"$user_id\"},{\"owner\":\"$user_id\"}]}" \
  "_id,channel_type,user,active,recipients,name,owner,description,icon,last_message_id,permissions,nsfw,server,role_permissions,default_permissions,voice,slowmode,forum"
export_collection server_members \
  "{\"_id.user\":\"$user_id\"}" \
  "_id,joined_at,nickname,pronouns,avatar,roles,timeout,can_publish,can_receive"
export_collection server_bans \
  "{\"_id.user\":\"$user_id\"}" \
  "_id,reason"
export_collection account_invites \
  "{\"claimed_by\":\"$user_id\"}" \
  "_id,used,claimed_by"
export_collection channel_invites \
  "{\"creator\":\"$user_id\"}" \
  "_id,type,server,channel,creator"
export_collection channel_webhooks \
  "{\"creator_id\":\"$user_id\"}" \
  "_id,name,avatar,creator_id,channel_id,permissions"
export_collection emojis \
  "{\"creator_id\":\"$user_id\"}" \
  "_id,parent,creator_id,name,animated,nsfw"
export_collection messages \
  "{\"author\":\"$user_id\"}" \
  "_id,channel,author,content,system,attachments,edited,embeds,replies,reactions,interactions,poll,pinned"
export_collection attachments \
  "{\"uploader_id\":\"$user_id\"}" \
  "_id,tag,filename,uploaded_at,uploader_id,used_for,deleted,reported,metadata,content_type,size,message_id,user_id,server_id,object_id"
export_collection server_members \
  "{\"_id.user\":\"$user_id\"}" \
  "_id,roles,joined_at,timeout"
export_collection user_settings \
  "{\"_id\":\"$user_id\"}" \
  "_id,flags,locale,ordering,appearance,notifications"
export_collection channel_unreads \
  "{\"_id.user\":\"$user_id\"}" \
  "_id,last_message_id,mentions"
export_collection user_xp \
  "{\"_id\":\"$user_id\"}" \
  "_id,xp,level"
export_collection user_tailslets \
  "{\"author_id\":\"$user_id\"}" \
  "_id,author_id,author_name,content,created_at"
export_collection user_videos \
  "{\"author_id\":\"$user_id\"}" \
  "_id,author_id,author_name,attachment_id,caption,created_at"
export_collection server_onboarding_completions \
  "{\"_id.user\":\"$user_id\"}" \
  "_id,answers,completed_at"
export_collection server_boosts \
  "{\"user_id\":\"$user_id\"}" \
  "_id,server_id,user_id,source,created_at"
export_collection premium_gifts \
  "{\"redeemed_by\":\"$user_id\"}" \
  "_id,duration_days,created_at,redeemed_by,redeemed_at,revoked"
export_collection safety_reports \
  "{\"author_id\":\"$user_id\"}" \
  "_id,author_id,content,additional_context,status,notes"
export_collection safety_snapshots \
  "{\"\$or\":[{\"content.message.author\":\"$user_id\"},{\"content.user._id\":\"$user_id\"}]}" \
  "_id,report_id,content"
export_collection safety_strikes \
  "{\"\$or\":[{\"user_id\":\"$user_id\"},{\"moderator_id\":\"$user_id\"}]}" \
  "_id,user_id,moderator_id,reason,created_at,expires_at"

tar -C "$staging" -czf "$archive" .
chmod 0600 "$archive"
echo "Created redacted export: $archive"
