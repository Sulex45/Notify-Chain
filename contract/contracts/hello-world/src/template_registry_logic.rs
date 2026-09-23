/// Template Registry Logic — Issue #352
///
/// Provides register/update/query operations for reusable notification templates.
/// Templates are stored in persistent storage keyed by a caller-supplied
/// `BytesN<32>` ID.  Only the original owner (creator) of a template may update
/// it.  Attempting to reference a non-existent template ID reverts with a clear
/// error.
use crate::base::errors::Error;
use crate::base::events::{
    AuthorizationFailure, NotificationCategory, NotificationPriority, TemplateRegistered,
    TemplateUpdated,
};
use crate::base::types::NotificationTemplate;
use soroban_sdk::{contracttype, Address, BytesN, Env, String};

// ============================================================================
// Limits
// ============================================================================

/// Maximum allowed byte-length for a template name.
const MAX_TEMPLATE_NAME_LEN: u32 = 100;

// ============================================================================
// Storage keys
// ============================================================================

/// Persistent-storage key variants used by the template registry.
///
/// Keys are separate from `DataKey` in `autoshare_logic` to avoid the two
/// enums merging into a single namespace and to keep each module's storage
/// footprint explicit.  Both enums live in persistent storage and are
/// distinguished by their variant discriminant at the serialisation layer.
#[contracttype]
pub enum TemplateKey {
    /// Full `NotificationTemplate` record keyed by template ID.
    Template(BytesN<32>),
}

// ============================================================================
// Public API
// ============================================================================

/// Register a new notification template on-chain.
///
/// The caller becomes the owner of the template and is the only address
/// authorised to update it in the future.
///
/// # Arguments
/// * `id`      – caller-chosen unique identifier; reverts with `AlreadyExists`
///               if it is already taken.
/// * `creator` – address that will own the template (must authorise the call).
/// * `name`    – human-readable label (max 100 bytes); reverts with
///               `TemplateNameTooLong` if exceeded.
/// * `content` – notification payload/body; must not be empty, reverts with
///               `TemplateContentEmpty` otherwise.
///
/// # Errors
/// - `AlreadyExists`        – a template with this `id` is already registered.
/// - `TemplateNameTooLong`  – `name` exceeds `MAX_TEMPLATE_NAME_LEN` bytes.
/// - `TemplateContentEmpty` – `content` is an empty string.
///
/// # Events
/// Emits [`TemplateRegistered`] on success.
pub fn register_template(
    env: Env,
    id: BytesN<32>,
    creator: Address,
    name: String,
    content: String,
) -> Result<(), Error> {
    creator.require_auth();

    let key = TemplateKey::Template(id.clone());

    // Prevent overwriting an existing template.
    if env.storage().persistent().has(&key) {
        return Err(Error::AlreadyExists);
    }

    validate_name(&name)?;
    validate_content(&content)?;

    let template = NotificationTemplate {
        id: id.clone(),
        owner: creator.clone(),
        name,
        content,
        created_at: env.ledger().timestamp(),
        updated_at: None,
    };

    env.storage().persistent().set(&key, &template);

    TemplateRegistered {
        owner: creator,
        category: NotificationCategory::Notification,
        priority: NotificationPriority::Medium,
        template_id: id,
    }
    .publish(&env);

    Ok(())
}

/// Update the `name` and/or `content` of an existing template.
///
/// Only the original owner of the template is permitted to call this function.
/// Any other caller reverts with `Unauthorized`.
///
/// # Arguments
/// * `id`     – identifier of the template to update.
/// * `caller` – must match the template's stored `owner`; must authorise the call.
/// * `name`   – replacement name (max 100 bytes).
/// * `content`– replacement content; must not be empty.
///
/// # Errors
/// - `TemplateNotFound`     – no template is registered under `id`.
/// - `Unauthorized`         – `caller` is not the template owner.
/// - `TemplateNameTooLong`  – replacement `name` exceeds `MAX_TEMPLATE_NAME_LEN`.
/// - `TemplateContentEmpty` – replacement `content` is empty.
///
/// # Events
/// Emits [`TemplateUpdated`] on success.
pub fn update_template(
    env: Env,
    id: BytesN<32>,
    caller: Address,
    name: String,
    content: String,
) -> Result<(), Error> {
    caller.require_auth();

    let key = TemplateKey::Template(id.clone());

    let mut template: NotificationTemplate = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::TemplateNotFound)?;

    // Only the original owner may update.
    if template.owner != caller {
        publish_authorization_failure(&env, &caller, "update_template");
        return Err(Error::Unauthorized);
    }

    validate_name(&name)?;
    validate_content(&content)?;

    template.name = name;
    template.content = content;
    template.updated_at = Some(env.ledger().timestamp());

    env.storage().persistent().set(&key, &template);

    TemplateUpdated {
        owner: caller,
        category: NotificationCategory::Notification,
        priority: NotificationPriority::Medium,
        template_id: id,
    }
    .publish(&env);

    Ok(())
}

/// Return the full record for a registered template.
///
/// # Errors
/// - `TemplateNotFound` – no template is registered under `id`.
pub fn get_template(env: Env, id: BytesN<32>) -> Result<NotificationTemplate, Error> {
    let key = TemplateKey::Template(id);
    env.storage()
        .persistent()
        .get(&key)
        .ok_or(Error::TemplateNotFound)
}

/// Return `true` if a template with the given `id` exists, `false` otherwise.
///
/// This is a pure view function and never reverts.
pub fn template_exists(env: Env, id: BytesN<32>) -> bool {
    let key = TemplateKey::Template(id);
    env.storage().persistent().has(&key)
}

// ============================================================================
// Validation helpers
// ============================================================================

fn validate_name(name: &String) -> Result<(), Error> {
    if name.len() > MAX_TEMPLATE_NAME_LEN {
        return Err(Error::TemplateNameTooLong);
    }
    Ok(())
}

fn validate_content(content: &String) -> Result<(), Error> {
    if content.len() == 0 {
        return Err(Error::TemplateContentEmpty);
    }
    Ok(())
}

// ============================================================================
// Auth helpers
// ============================================================================

fn publish_authorization_failure(env: &Env, caller: &Address, action: &str) {
    AuthorizationFailure {
        caller: caller.clone(),
        category: NotificationCategory::Admin,
        priority: NotificationPriority::Critical,
        action: String::from_str(env, action),
    }
    .publish(env);
}
/// Notification channel creation, subscription, and subscriber-count views.
use crate::base::channel::{
    is_subscribed, load_channel, load_subscribers, push_all_channel_id, save_channel,
    save_subscribers, set_subscribed, BatchSubscribeCompleted, BatchSubscribeResult,
    ChannelCreated, ChannelSubscribed, ChannelUnsubscribed, NotificationChannel,
    MAX_BATCH_SUBSCRIBE, MAX_CHANNEL_NAME_LENGTH,
};
use crate::base::errors::Error;
use crate::base::events::{NotificationCategory, NotificationPriority};
use soroban_sdk::{Address, BytesN, Env, String, Vec};

fn require_not_paused(env: &Env) -> Result<(), Error> {
    if crate::autoshare_logic::get_paused_status(env) {
        return Err(Error::ContractPaused);
    }
    Ok(())
}

/// Creates a new notification channel owned by `creator`.
///
/// Stores the creator address permanently so it can be queried later.
/// Emits a [`ChannelCreated`] event.
pub fn create_channel(
    env: Env,
    id: BytesN<32>,
    name: String,
    creator: Address,
) -> Result<(), Error> {
    creator.require_auth();
    require_not_paused(&env)?;

    if name.is_empty() {
        return Err(Error::InvalidInput);
    }
    if name.len() > MAX_CHANNEL_NAME_LENGTH {
        return Err(Error::NameTooLong);
    }

    if load_channel(&env, &id).is_some() {
        return Err(Error::AlreadyExists);
    }

    let channel = NotificationChannel {
        id: id.clone(),
        creator: creator.clone(),
        name,
        subscriber_count: 0,
        is_active: true,
        created_at: env.ledger().timestamp(),
    };
    save_channel(&env, &channel);
    push_all_channel_id(&env, &id);

    ChannelCreated {
        creator,
        category: NotificationCategory::Notification,
        priority: NotificationPriority::Medium,
        channel_id: id,
    }
    .publish(&env);

    Ok(())
}

/// Returns channel metadata including creator and subscriber count.
pub fn get_channel(env: Env, id: BytesN<32>) -> Result<NotificationChannel, Error> {
    load_channel(&env, &id).ok_or(Error::NotFound)
}

/// Returns the wallet address that originally created the channel.
pub fn get_channel_creator(env: Env, id: BytesN<32>) -> Result<Address, Error> {
    Ok(load_channel(&env, &id).ok_or(Error::NotFound)?.creator)
}

/// Read-only view of the active subscriber count for a channel.
pub fn get_subscriber_count(env: Env, id: BytesN<32>) -> Result<u32, Error> {
    Ok(load_channel(&env, &id)
        .ok_or(Error::NotFound)?
        .subscriber_count)
}

/// Returns whether `subscriber` is currently subscribed to the channel.
pub fn is_channel_subscriber(env: Env, id: BytesN<32>, subscriber: Address) -> bool {
    is_subscribed(&env, &id, &subscriber)
}

/// Subscribe a single address to a channel.
pub fn subscribe(env: Env, channel_id: BytesN<32>, subscriber: Address) -> Result<(), Error> {
    subscriber.require_auth();
    require_not_paused(&env)?;
    subscribe_one(&env, &channel_id, &subscriber)?;
    Ok(())
}

/// Unsubscribe from a channel. No-ops the count if not currently subscribed.
pub fn unsubscribe(env: Env, channel_id: BytesN<32>, subscriber: Address) -> Result<(), Error> {
    subscriber.require_auth();
    require_not_paused(&env)?;

    let mut channel = load_channel(&env, &channel_id).ok_or(Error::NotFound)?;

    if !is_subscribed(&env, &channel_id, &subscriber) {
        return Err(Error::NotFound);
    }

    set_subscribed(&env, &channel_id, &subscriber, false);

    let mut subscribers = load_subscribers(&env, &channel_id);
    let mut next = Vec::new(&env);
    for addr in subscribers.iter() {
        if addr != subscriber {
            next.push_back(addr);
        }
    }
    save_subscribers(&env, &channel_id, &next);

    channel.subscriber_count = channel.subscriber_count.saturating_sub(1);
    save_channel(&env, &channel);

    ChannelUnsubscribed {
        channel_id,
        subscriber,
        category: NotificationCategory::Notification,
        subscriber_count: channel.subscriber_count,
    }
    .publish(&env);

    Ok(())
}

/// Subscribe to multiple channels in a single transaction.
///
/// Failed individual subscriptions (missing channel, inactive, already
/// subscribed) are skipped and do not roll back successful ones — state for
/// failed entries is left unchanged. Structural errors (empty batch, too large,
/// paused contract) abort the whole call before any mutation.
///
/// # Gas notes
/// See `docs/BATCH_SUBSCRIBE_GAS.md`. Batching N subscriptions into one
/// transaction avoids N separate transaction base fees and repeated auth
/// overhead. Per-channel storage writes still scale linearly with N.
pub fn batch_subscribe(
    env: Env,
    channel_ids: Vec<BytesN<32>>,
    subscriber: Address,
) -> Result<BatchSubscribeResult, Error> {
    subscriber.require_auth();
    require_not_paused(&env)?;

    let count = channel_ids.len();
    if count == 0 {
        return Err(Error::InvalidInput);
    }
    if count > MAX_BATCH_SUBSCRIBE {
        return Err(Error::BatchTooLarge);
    }

    let mut succeeded = 0u32;
    let mut failed = 0u32;
    let mut subscribed_ids = Vec::new(&env);

    for i in 0..count {
        let channel_id = channel_ids.get(i).unwrap();
        match subscribe_one(&env, &channel_id, &subscriber) {
            Ok(()) => {
                succeeded += 1;
                subscribed_ids.push_back(channel_id);
            }
            Err(_) => {
                failed += 1;
            }
        }
    }

    BatchSubscribeCompleted {
        subscriber: subscriber.clone(),
        category: NotificationCategory::Notification,
        priority: NotificationPriority::Medium,
        succeeded,
        failed,
    }
    .publish(&env);

    Ok(BatchSubscribeResult {
        succeeded,
        failed,
        subscribed_ids,
    })
}

fn subscribe_one(
    env: &Env,
    channel_id: &BytesN<32>,
    subscriber: &Address,
) -> Result<(), Error> {
    let mut channel = load_channel(env, channel_id).ok_or(Error::NotFound)?;

    if !channel.is_active {
        return Err(Error::GroupInactive);
    }

    if is_subscribed(env, channel_id, subscriber) {
        return Err(Error::AlreadyExists);
    }

    set_subscribed(env, channel_id, subscriber, true);

    let mut subscribers = load_subscribers(env, channel_id);
    subscribers.push_back(subscriber.clone());
    save_subscribers(env, channel_id, &subscribers);

    channel.subscriber_count = channel
        .subscriber_count
        .checked_add(1)
        .ok_or(Error::InvalidInput)?;
    save_channel(env, &channel);

    ChannelSubscribed {
        channel_id: channel_id.clone(),
        subscriber: subscriber.clone(),
        category: NotificationCategory::Notification,
        subscriber_count: channel.subscriber_count,
    }
    .publish(env);

    Ok(())
}
