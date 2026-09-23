use soroban_sdk::{contract, contractimpl, Address, Env, String, Symbol};

#[contract]
pub struct MockToken;

#[contractimpl]
impl MockToken {
    pub fn initialize(env: Env, admin: Address, decimal: u32, name: String, symbol: String) {
        if env.storage().instance().has(&Symbol::new(&env, "admin")) {
            panic!("Already initialized");
        }
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "admin"), &admin);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "decimal"), &decimal);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "name"), &name);
        env.storage()
            .instance()
            .set(&Symbol::new(&env, "symbol"), &symbol);
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        if amount <= 0 {
            panic!("Invalid amount");
        }
        let admin: Address = env
            .storage()
            .instance()
            .get(&Symbol::new(&env, "admin"))
            .unwrap();
        admin.require_auth();

        let key = (&to,);
        let mut balance: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        balance += amount;
        env.storage().persistent().set(&key, &balance);

        // Update total supply
        let supply_key = Symbol::new(&env, "total_supply");
        let mut supply: i128 = env.storage().instance().get(&supply_key).unwrap_or(0);
        supply += amount;
        env.storage().instance().set(&supply_key, &supply);
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        if amount <= 0 {
            panic!("Invalid amount");
        }

        let from_key = (&from,);
        let mut from_balance: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);

        if from_balance < amount {
            panic!("Insufficient balance");
        }

        from_balance -= amount;
        env.storage().persistent().set(&from_key, &from_balance);

        let to_key = (&to,);
        let mut to_balance: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        to_balance += amount;
        env.storage().persistent().set(&to_key, &to_balance);
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        let key = (&id,);
        env.storage().persistent().get(&key).unwrap_or(0)
    }

    pub fn decimals(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "decimal"))
            .unwrap_or(0)
    }

    pub fn name(env: Env) -> String {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "name"))
            .unwrap()
    }

    pub fn symbol(env: Env) -> String {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "symbol"))
            .unwrap()
    }

    pub fn total_supply(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&Symbol::new(&env, "total_supply"))
            .unwrap_or(0)
    }
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
