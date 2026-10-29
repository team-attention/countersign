//! Countersign: a policy wallet for an AI agent.
//!
//! The agent can only *ask* for a payment. The contract decides:
//! - `auto_paid`: receiver is allowlisted and the amount fits the per-payment and daily limits.
//! - `pending`:   anything else that is not obviously bad. Only the human owner can approve it.
//! - `blocked`:   receiver is on the blocklist, the wallet is paused, or funds are short.
//!
//! The intended deployment gives the agent nothing but function-call access keys on this
//! account (see `add_agent_key`), and removes every full-access key after setup. A function-call
//! key cannot transfer NEAR, cannot attach a deposit and cannot add keys, so the only way money
//! leaves this account is through the rules below.

use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::store::IterableMap;
use near_sdk::{
    AccountId, Allowance, BorshStorageKey, NearToken, PanicOnDefault, Promise,
    PublicKey, env, near, require,
};

const DAY_NS: u64 = 86_400 * 1_000_000_000;
const MAX_REASON_LEN: usize = 280;
/// Balance kept on the account for storage; the agent can never spend it.
const STORAGE_RESERVE: NearToken = NearToken::from_millinear(500);

#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
enum StorageKey {
    Requests,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Debug)]
pub struct Policy {
    /// Largest single payment that can go out without the owner.
    pub per_payment_limit: U128,
    /// Total that can go out without the owner within one rolling 24h window.
    pub daily_limit: U128,
    /// Receivers that may be paid automatically (still subject to the limits).
    pub allowlist: Vec<AccountId>,
    /// Receivers that are refused outright, even with owner approval.
    pub blocklist: Vec<AccountId>,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    AutoPaid,
    Pending,
    Approved,
    Rejected,
    Blocked,
    /// The transfer was sent but failed on the receiver side; funds came back.
    Failed,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Debug)]
pub struct PaymentRequest {
    pub id: u64,
    pub receiver: AccountId,
    pub amount: U128,
    /// The agent's own explanation, stored verbatim (trimmed to 280 chars).
    pub reason: String,
    pub status: Status,
    /// Why the contract (or the owner) decided what it decided.
    pub notes: Vec<String>,
    pub created_at_ns: u64,
    pub decided_at_ns: Option<u64>,
}

#[near(serializers = [json])]
pub struct WalletStatus {
    pub owner: AccountId,
    pub agent: AccountId,
    pub paused: bool,
    pub balance: U128,
    pub spendable: U128,
    pub auto_spent_today: U128,
    pub window_started_at_ns: u64,
    pub pending: u64,
    pub total_requests: u64,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Countersign {
    owner: AccountId,
    agent: AccountId,
    policy: Policy,
    paused: bool,
    requests: IterableMap<u64, PaymentRequest>,
    next_id: u64,
    window_start_ns: u64,
    auto_spent_in_window: u128,
    /// Amount promised to in-flight transfers, so two quick requests cannot overspend.
    in_flight: u128,
}

/// 1234500000000000000000000 -> "1.2345 NEAR" (trailing zeros trimmed, max 4 decimals).
fn near_str(yocto: u128) -> String {
    let unit: u128 = 10u128.pow(24);
    let whole = yocto / unit;
    let frac = (yocto % unit) / 10u128.pow(20);
    if frac == 0 {
        return format!("{whole} NEAR");
    }
    let f = format!("{frac:04}");
    format!("{whole}.{} NEAR", f.trim_end_matches('0'))
}

fn emit(event: &str, data: near_sdk::serde_json::Value) {
    env::log_str(&format!(
        "EVENT_JSON:{}",
        json!({"standard": "countersign", "version": "1.0.0", "event": event, "data": [data]})
    ));
}

#[near]
impl Countersign {
    #[init]
    pub fn new(owner: AccountId, agent: AccountId, policy: Policy) -> Self {
        require!(owner != agent, "owner and agent must be different accounts");
        Self::check_policy(&policy);
        Self {
            owner,
            agent,
            policy,
            paused: false,
            requests: IterableMap::new(StorageKey::Requests),
            next_id: 0,
            window_start_ns: env::block_timestamp(),
            auto_spent_in_window: 0,
            in_flight: 0,
        }
    }

    // ---------------------------------------------------------------- agent

    /// The only method the agent's key is allowed to call.
    /// Never panics on a policy decision: the decision is recorded and returned.
    pub fn request_payment(
        &mut self,
        receiver: AccountId,
        amount: U128,
        reason: String,
    ) -> PaymentRequest {
        require!(
            env::predecessor_account_id() == self.agent,
            "only the agent can request payments"
        );
        require!(amount.0 > 0, "amount must be positive");
        self.roll_window();

        let mut reason = reason;
        if reason.len() > MAX_REASON_LEN {
            let mut cut = MAX_REASON_LEN;
            while !reason.is_char_boundary(cut) {
                cut -= 1;
            }
            reason.truncate(cut);
        }

        let id = self.next_id;
        self.next_id += 1;
        let mut req = PaymentRequest {
            id,
            receiver: receiver.clone(),
            amount,
            reason,
            status: Status::Pending,
            notes: vec![],
            created_at_ns: env::block_timestamp(),
            decided_at_ns: None,
        };

        let mut block = vec![];
        if self.paused {
            block.push("wallet is paused by the owner".to_string());
        }
        if self.policy.blocklist.contains(&receiver) {
            block.push(format!("{receiver} is on the blocklist"));
        }
        if receiver == self.agent || receiver == env::current_account_id() {
            block.push("cannot pay the wallet or the agent itself".to_string());
        }
        if amount.0 > self.spendable() {
            block.push(format!(
                "amount {} exceeds spendable balance {}",
                near_str(amount.0),
                near_str(self.spendable())
            ));
        }

        if !block.is_empty() {
            req.status = Status::Blocked;
            req.notes = block;
            req.decided_at_ns = Some(env::block_timestamp());
            emit("payment_blocked", json!(&req));
            self.requests.insert(id, req.clone());
            return req;
        }

        let mut hold = vec![];
        if !self.policy.allowlist.contains(&receiver) {
            hold.push(format!("{receiver} is not on the allowlist"));
        }
        if amount.0 > self.policy.per_payment_limit.0 {
            hold.push(format!(
                "amount {} is over the per-payment limit {}",
                near_str(amount.0),
                near_str(self.policy.per_payment_limit.0)
            ));
        }
        if self.auto_spent_in_window + amount.0 > self.policy.daily_limit.0 {
            hold.push(format!(
                "would bring today's automatic spend to {}, over the daily limit {}",
                near_str(self.auto_spent_in_window + amount.0),
                near_str(self.policy.daily_limit.0)
            ));
        }

        if hold.is_empty() {
            req.status = Status::AutoPaid;
            req.notes = vec!["allowlisted receiver within limits".to_string()];
            req.decided_at_ns = Some(env::block_timestamp());
            self.auto_spent_in_window += amount.0;
            self.requests.insert(id, req.clone());
            emit("payment_auto_paid", json!(&req));
            self.send(id, receiver, amount.0);
        } else {
            hold.push("waiting for the owner".to_string());
            req.notes = hold;
            self.requests.insert(id, req.clone());
            emit("payment_pending", json!(&req));
        }
        req
    }

    // ---------------------------------------------------------------- owner

    /// Owner approves a pending request. Requires 1 yoctoNEAR so it can only be signed with a
    /// full-access key (function-call keys cannot attach deposits).
    #[payable]
    pub fn approve(&mut self, id: u64, note: Option<String>) -> PaymentRequest {
        self.assert_owner();
        let mut req = self.pending(id);
        require!(
            !self.policy.blocklist.contains(&req.receiver),
            "receiver is on the blocklist; remove it from the policy first"
        );
        require!(
            req.amount.0 <= self.spendable(),
            "not enough spendable balance"
        );
        req.status = Status::Approved;
        req.notes
            .push(note.unwrap_or_else(|| "approved by owner".to_string()));
        req.decided_at_ns = Some(env::block_timestamp());
        self.requests.insert(id, req.clone());
        emit("payment_approved", json!(&req));
        self.send(id, req.receiver.clone(), req.amount.0);
        req
    }

    #[payable]
    pub fn reject(&mut self, id: u64, note: Option<String>) -> PaymentRequest {
        self.assert_owner();
        let mut req = self.pending(id);
        req.status = Status::Rejected;
        req.notes
            .push(note.unwrap_or_else(|| "rejected by owner".to_string()));
        req.decided_at_ns = Some(env::block_timestamp());
        self.requests.insert(id, req.clone());
        emit("payment_rejected", json!(&req));
        req
    }

    #[payable]
    pub fn set_policy(&mut self, policy: Policy) {
        self.assert_owner();
        Self::check_policy(&policy);
        self.policy = policy;
        emit("policy_updated", json!(&self.policy));
    }

    /// Kill switch: while paused every agent request is blocked.
    #[payable]
    pub fn set_paused(&mut self, paused: bool) {
        self.assert_owner();
        self.paused = paused;
        emit("paused", json!({"paused": paused}));
    }

    /// Give the agent a function-call key on this account. `receiver_id` is either this wallet
    /// (method must be `request_payment`) or an outside contract the agent works with.
    #[payable]
    pub fn add_agent_key(
        &mut self,
        public_key: PublicKey,
        receiver_id: AccountId,
        method_names: Vec<String>,
        gas_allowance: U128,
    ) -> Promise {
        self.assert_owner();
        require!(
            self.agent == env::current_account_id(),
            "agent keys live on the wallet account only when agent == wallet"
        );
        require!(!method_names.is_empty(), "list at least one method");
        if receiver_id == env::current_account_id() {
            require!(
                method_names == vec!["request_payment".to_string()],
                "on the wallet itself the agent key may only call request_payment"
            );
        }
        let allowance = Allowance::limited(NearToken::from_yoctonear(gas_allowance.0))
            .unwrap_or_else(|| env::panic_str("gas_allowance must be positive"));
        emit(
            "agent_key_added",
            json!({"public_key": public_key, "receiver_id": receiver_id, "method_names": method_names}),
        );
        Promise::new(env::current_account_id()).add_access_key_allowance(
            public_key,
            allowance,
            receiver_id,
            method_names.join(","),
        )
    }

    #[payable]
    pub fn revoke_key(&mut self, public_key: PublicKey) -> Promise {
        self.assert_owner();
        emit("key_revoked", json!({"public_key": public_key}));
        Promise::new(env::current_account_id()).delete_key(public_key)
    }

    // ---------------------------------------------------------------- callbacks

    #[private]
    pub fn on_transfer(&mut self, id: u64, amount: U128, auto: bool) -> bool {
        self.in_flight = self.in_flight.saturating_sub(amount.0);
        let ok = env::promise_result_checked(0, 64).is_ok();
        if !ok {
            if let Some(mut req) = self.requests.get(&id).cloned() {
                req.status = Status::Failed;
                req.notes
                    .push("transfer failed; funds returned to the wallet".to_string());
                if auto {
                    self.auto_spent_in_window = self.auto_spent_in_window.saturating_sub(amount.0);
                }
                self.requests.insert(id, req.clone());
                emit("payment_failed", json!(&req));
            }
        }
        ok
    }

    // ---------------------------------------------------------------- views

    pub fn get_policy(&self) -> Policy {
        self.policy.clone()
    }

    pub fn get_request(&self, id: u64) -> Option<PaymentRequest> {
        self.requests.get(&id).cloned()
    }

    /// Newest first.
    pub fn list_requests(&self, from: Option<u64>, limit: Option<u64>) -> Vec<PaymentRequest> {
        let limit = limit.unwrap_or(50).min(200);
        let start = from.unwrap_or(self.next_id.saturating_sub(1));
        (0..=start)
            .rev()
            .filter_map(|i| self.requests.get(&i).cloned())
            .take(limit as usize)
            .collect()
    }

    pub fn list_pending(&self) -> Vec<PaymentRequest> {
        self.requests
            .values()
            .filter(|r| r.status == Status::Pending)
            .cloned()
            .collect()
    }

    pub fn get_status(&self) -> WalletStatus {
        let window_fresh = env::block_timestamp() < self.window_start_ns + DAY_NS;
        WalletStatus {
            owner: self.owner.clone(),
            agent: self.agent.clone(),
            paused: self.paused,
            balance: U128(env::account_balance().as_yoctonear()),
            spendable: U128(self.spendable()),
            auto_spent_today: U128(if window_fresh {
                self.auto_spent_in_window
            } else {
                0
            }),
            window_started_at_ns: self.window_start_ns,
            pending: self
                .requests
                .values()
                .filter(|r| r.status == Status::Pending)
                .count() as u64,
            total_requests: self.next_id,
        }
    }

    // ---------------------------------------------------------------- internal

    fn send(&mut self, id: u64, receiver: AccountId, amount: u128) {
        let auto = self
            .requests
            .get(&id)
            .map(|r| r.status == Status::AutoPaid)
            .unwrap_or(false);
        self.in_flight += amount;
        Promise::new(receiver)
            .transfer(NearToken::from_yoctonear(amount))
            .then(Self::ext(env::current_account_id()).on_transfer(id, U128(amount), auto))
            .detach();
    }

    fn spendable(&self) -> u128 {
        let storage = env::storage_byte_cost()
            .saturating_mul(env::storage_usage() as u128)
            .as_yoctonear();
        env::account_balance()
            .as_yoctonear()
            .saturating_sub(storage)
            .saturating_sub(STORAGE_RESERVE.as_yoctonear())
            .saturating_sub(self.in_flight)
    }

    fn roll_window(&mut self) {
        if env::block_timestamp() >= self.window_start_ns + DAY_NS {
            self.window_start_ns = env::block_timestamp();
            self.auto_spent_in_window = 0;
        }
    }

    fn pending(&self, id: u64) -> PaymentRequest {
        let req = self
            .requests
            .get(&id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("no such request"));
        require!(req.status == Status::Pending, "request is not pending");
        req
    }

    fn assert_owner(&self) {
        require!(
            env::predecessor_account_id() == self.owner,
            "only the owner can do this"
        );
        require!(
            env::attached_deposit() == NearToken::from_yoctonear(1),
            "attach exactly 1 yoctoNEAR (full-access key required)"
        );
    }

    fn check_policy(p: &Policy) {
        require!(
            p.per_payment_limit.0 <= p.daily_limit.0,
            "per-payment limit cannot exceed the daily limit"
        );
        require!(
            p.allowlist.iter().all(|a| !p.blocklist.contains(a)),
            "an account cannot be on both lists"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{VMContextBuilder, accounts};
    use near_sdk::testing_env;

    const NEAR: u128 = 1_000_000_000_000_000_000_000_000;

    fn ctx(predecessor: AccountId, deposit: u128) -> VMContextBuilder {
        let mut b = VMContextBuilder::new();
        b.current_account_id("wallet.near".parse().unwrap())
            .predecessor_account_id(predecessor)
            .account_balance(NearToken::from_yoctonear(10 * NEAR))
            .attached_deposit(NearToken::from_yoctonear(deposit));
        b
    }

    fn wallet() -> Countersign {
        testing_env!(ctx(accounts(0), 0).build());
        Countersign::new(
            accounts(0), // owner: alice
            accounts(1), // agent: bob
            Policy {
                per_payment_limit: U128(NEAR / 2),
                daily_limit: U128(NEAR),
                allowlist: vec![accounts(2)],
                blocklist: vec![accounts(3)],
            },
        )
    }

    fn as_agent() {
        testing_env!(ctx(accounts(1), 0).build());
    }

    #[test]
    fn allowlisted_small_payment_is_auto_paid() {
        let mut w = wallet();
        as_agent();
        let r = w.request_payment(accounts(2), U128(NEAR / 10), "compute".into());
        assert_eq!(r.status, Status::AutoPaid);
        assert_eq!(w.get_status().auto_spent_today.0, NEAR / 10);
    }

    #[test]
    fn unknown_receiver_waits_for_owner() {
        let mut w = wallet();
        as_agent();
        let r = w.request_payment(accounts(4), U128(NEAR / 10), "license fee".into());
        assert_eq!(r.status, Status::Pending);
        assert!(r.notes[0].contains("not on the allowlist"));
    }

    #[test]
    fn blocklisted_receiver_is_blocked() {
        let mut w = wallet();
        as_agent();
        let r = w.request_payment(accounts(3), U128(1), "x".into());
        assert_eq!(r.status, Status::Blocked);
    }

    #[test]
    fn over_limit_waits_and_daily_limit_holds() {
        let mut w = wallet();
        as_agent();
        assert_eq!(
            w.request_payment(accounts(2), U128(NEAR * 6 / 10), "big".into()).status,
            Status::Pending
        );
        assert_eq!(
            w.request_payment(accounts(2), U128(NEAR / 2), "a".into()).status,
            Status::AutoPaid
        );
        assert_eq!(
            w.request_payment(accounts(2), U128(NEAR / 2), "b".into()).status,
            Status::AutoPaid
        );
        let third = w.request_payment(accounts(2), U128(1), "c".into());
        assert_eq!(third.status, Status::Pending);
        assert!(third.notes.iter().any(|n| n.contains("daily limit")));
    }

    #[test]
    fn paused_blocks_everything() {
        let mut w = wallet();
        testing_env!(ctx(accounts(0), 1).build());
        w.set_paused(true);
        as_agent();
        assert_eq!(
            w.request_payment(accounts(2), U128(1), "x".into()).status,
            Status::Blocked
        );
    }

    #[test]
    #[should_panic(expected = "only the agent")]
    fn strangers_cannot_request() {
        let mut w = wallet();
        testing_env!(ctx(accounts(4), 0).build());
        w.request_payment(accounts(2), U128(1), "x".into());
    }

    #[test]
    #[should_panic(expected = "only the owner")]
    fn agent_cannot_approve() {
        let mut w = wallet();
        as_agent();
        w.request_payment(accounts(4), U128(1), "x".into());
        testing_env!(ctx(accounts(1), 1).build());
        w.approve(0, None);
    }

    #[test]
    #[should_panic(expected = "attach exactly 1 yoctoNEAR")]
    fn owner_needs_full_access_key() {
        let mut w = wallet();
        as_agent();
        w.request_payment(accounts(4), U128(1), "x".into());
        testing_env!(ctx(accounts(0), 0).build());
        w.approve(0, None);
    }

    #[test]
    fn owner_approves_pending() {
        let mut w = wallet();
        as_agent();
        w.request_payment(accounts(4), U128(NEAR / 10), "x".into());
        testing_env!(ctx(accounts(0), 1).build());
        let r = w.approve(0, Some("ok".into()));
        assert_eq!(r.status, Status::Approved);
        assert!(w.list_pending().is_empty());
    }

    #[test]
    fn near_strings() {
        assert_eq!(near_str(NEAR), "1 NEAR");
        assert_eq!(near_str(NEAR * 55 / 100), "0.55 NEAR");
        assert_eq!(near_str(NEAR / 20), "0.05 NEAR");
        assert_eq!(near_str(0), "0 NEAR");
    }

    #[test]
    fn long_reason_is_trimmed_on_char_boundary() {
        let mut w = wallet();
        as_agent();
        let r = w.request_payment(accounts(2), U128(1), "가".repeat(200));
        assert!(r.reason.len() <= MAX_REASON_LEN);
    }
}
