//! Bounty board: a client locks NEAR against a coding task, a worker submits a patch hash,
//! the client checks the work off-chain (runs the hidden tests) and accepts or rejects.
//! Accepting pays the reward to the worker's payout account (for the demo: the agent's
//! Countersign wallet), so the agent's earnings land under its spending policy.

use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::store::IterableMap;
use near_sdk::{
    AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise, env, near, require,
};

#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
enum StorageKey {
    Bounties,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Debug, PartialEq)]
pub enum BountyStatus {
    Open,
    Submitted,
    Paid,
    Cancelled,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Debug)]
pub struct Bounty {
    pub id: u64,
    pub client: AccountId,
    pub title: String,
    /// Where the task lives, e.g. a repo path or URL plus the visible test command.
    pub spec: String,
    pub reward: U128,
    pub status: BountyStatus,
    pub worker: Option<AccountId>,
    pub payout_to: Option<AccountId>,
    /// e.g. "sha256:<hex of the patch>"
    pub submission: Option<String>,
    pub notes: Vec<String>,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct BountyBoard {
    bounties: IterableMap<u64, Bounty>,
    next_id: u64,
}

const MAX_TEXT: usize = 1000;
const CALLBACK_GAS: Gas = Gas::from_tgas(10);

fn emit(event: &str, data: near_sdk::serde_json::Value) {
    env::log_str(&format!(
        "EVENT_JSON:{}",
        json!({"standard": "bounty-board", "version": "1.0.0", "event": event, "data": [data]})
    ));
}

#[near]
impl BountyBoard {
    #[init]
    pub fn new() -> Self {
        Self {
            bounties: IterableMap::new(StorageKey::Bounties),
            next_id: 0,
        }
    }

    /// Lock the attached deposit as the reward.
    #[payable]
    pub fn post_bounty(&mut self, title: String, spec: String) -> Bounty {
        let reward = env::attached_deposit();
        require!(reward > NearToken::from_millinear(10), "reward must be over 0.01 NEAR");
        require!(title.len() <= 120 && spec.len() <= MAX_TEXT, "text too long");
        let b = Bounty {
            id: self.next_id,
            client: env::predecessor_account_id(),
            title,
            spec,
            reward: U128(reward.as_yoctonear()),
            status: BountyStatus::Open,
            worker: None,
            payout_to: None,
            submission: None,
            notes: vec![],
        };
        self.next_id += 1;
        self.bounties.insert(b.id, b.clone());
        emit("bounty_posted", json!(&b));
        b
    }

    /// Callable with a function-call key (no deposit). The caller is the worker and
    /// `payout_to` must be the caller: a worker key cannot route the reward elsewhere
    /// (for the demo the caller is the agent's Countersign wallet, so earnings stay under
    /// its spending policy). Checked before the bounty lookup.
    pub fn submit_work(&mut self, id: u64, payout_to: AccountId, submission: String) -> Bounty {
        require!(
            payout_to == env::predecessor_account_id(),
            "payout_to must be the submitting account"
        );
        require!(submission.len() <= 200, "submission too long");
        let mut b = self.get(id);
        require!(b.status == BountyStatus::Open, "bounty is not open");
        b.status = BountyStatus::Submitted;
        b.worker = Some(env::predecessor_account_id());
        b.payout_to = Some(payout_to);
        b.submission = Some(submission);
        self.bounties.insert(id, b.clone());
        emit("work_submitted", json!(&b));
        b
    }

    /// Client accepts the submission and the reward is paid out. If the transfer fails
    /// (e.g. the payout account was deleted), `on_payout` puts the bounty back to
    /// `Submitted` so the client can accept again or reject; the reward stays on the board.
    pub fn accept(&mut self, id: u64, note: Option<String>) -> Bounty {
        let mut b = self.get(id);
        require!(env::predecessor_account_id() == b.client, "only the client can accept");
        require!(b.status == BountyStatus::Submitted, "nothing submitted");
        b.status = BountyStatus::Paid;
        b.notes.push(note.unwrap_or_else(|| "accepted".into()));
        self.bounties.insert(id, b.clone());
        emit("bounty_paid", json!(&b));
        Promise::new(b.payout_to.clone().unwrap())
            .transfer(NearToken::from_yoctonear(b.reward.0))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .on_payout(id, true),
            )
            .detach();
        b
    }

    /// Client sends the work back; the bounty reopens.
    pub fn reject_work(&mut self, id: u64, note: String) -> Bounty {
        let mut b = self.get(id);
        require!(env::predecessor_account_id() == b.client, "only the client can reject");
        require!(b.status == BountyStatus::Submitted, "nothing submitted");
        b.status = BountyStatus::Open;
        b.notes.push(format!("rejected: {note}"));
        b.worker = None;
        b.payout_to = None;
        b.submission = None;
        self.bounties.insert(id, b.clone());
        emit("work_rejected", json!(&b));
        b
    }

    /// Client takes an open bounty back. A failed refund reopens it (see `on_payout`).
    pub fn cancel(&mut self, id: u64) -> Bounty {
        let mut b = self.get(id);
        require!(env::predecessor_account_id() == b.client, "only the client can cancel");
        require!(b.status == BountyStatus::Open, "only open bounties can be cancelled");
        b.status = BountyStatus::Cancelled;
        self.bounties.insert(id, b.clone());
        emit("bounty_cancelled", json!(&b));
        Promise::new(b.client.clone())
            .transfer(NearToken::from_yoctonear(b.reward.0))
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .on_payout(id, false),
            )
            .detach();
        b
    }

    /// Runs after a payout (`accept`) or refund (`cancel`). On failure the NEAR is back on
    /// the board, so the bounty returns to the state it was in before the transfer.
    #[private]
    pub fn on_payout(&mut self, id: u64, accepted: bool) -> bool {
        let ok = env::promise_result_checked(0, 64).is_ok();
        if !ok {
            let mut b = self.get(id);
            if accepted {
                b.status = BountyStatus::Submitted;
                b.notes.push("payout failed; reward back on the board".into());
            } else {
                b.status = BountyStatus::Open;
                b.notes.push("refund failed; bounty reopened".into());
            }
            self.bounties.insert(id, b.clone());
            emit("payout_failed", json!(&b));
        }
        ok
    }

    pub fn get_bounty(&self, id: u64) -> Option<Bounty> {
        self.bounties.get(&id).cloned()
    }

    pub fn list_bounties(&self, status: Option<BountyStatus>) -> Vec<Bounty> {
        self.bounties
            .values()
            .filter(|b| status.as_ref().is_none_or(|s| &b.status == s))
            .cloned()
            .collect()
    }

    fn get(&self, id: u64) -> Bounty {
        self.bounties
            .get(&id)
            .cloned()
            .unwrap_or_else(|| env::panic_str("no such bounty"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{VMContextBuilder, accounts};
    use near_sdk::testing_env;
    use near_sdk::{PromiseResult, RuntimeFeesConfig, test_vm_config};

    fn as_(who: AccountId, deposit: NearToken) {
        testing_env!(
            VMContextBuilder::new()
                .predecessor_account_id(who)
                .attached_deposit(deposit)
                .build()
        );
    }

    #[test]
    fn full_cycle() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
        as_(accounts(1), NearToken::from_yoctonear(0));
        c.submit_work(0, accounts(1), "sha256:ab".into());
        as_(accounts(0), NearToken::from_yoctonear(0));
        let b = c.accept(0, None);
        assert_eq!(b.status, BountyStatus::Paid);
        assert_eq!(b.payout_to, Some(accounts(1)));
    }

    #[test]
    #[should_panic(expected = "payout_to must be the submitting account")]
    fn worker_cannot_route_reward_elsewhere() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
        as_(accounts(1), NearToken::from_yoctonear(0));
        c.submit_work(0, accounts(2), "sha256:ab".into());
    }

    #[test]
    #[should_panic(expected = "payout_to must be the submitting account")]
    fn payout_check_comes_before_bounty_lookup() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        as_(accounts(1), NearToken::from_yoctonear(0));
        c.submit_work(7, accounts(2), "sha256:ab".into());
    }

    #[test]
    #[should_panic(expected = "only the client")]
    fn worker_cannot_accept_own_work() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
        as_(accounts(1), NearToken::from_yoctonear(0));
        c.submit_work(0, accounts(1), "sha256:ab".into());
        c.accept(0, None);
    }

    fn callback(result: PromiseResult) {
        let ctx = VMContextBuilder::new()
            .current_account_id(accounts(3))
            .predecessor_account_id(accounts(3))
            .build();
        testing_env!(
            ctx,
            test_vm_config(),
            RuntimeFeesConfig::test(),
            Default::default(),
            vec![result]
        );
    }

    fn accepted_board() -> BountyBoard {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
        as_(accounts(1), NearToken::from_yoctonear(0));
        c.submit_work(0, accounts(1), "sha256:ab".into());
        as_(accounts(0), NearToken::from_yoctonear(0));
        c.accept(0, None);
        c
    }

    #[test]
    fn failed_payout_returns_to_submitted_and_can_be_accepted_again() {
        let mut c = accepted_board();
        callback(PromiseResult::Failed);
        assert!(!c.on_payout(0, true));
        let b = c.get_bounty(0).unwrap();
        assert_eq!(b.status, BountyStatus::Submitted);
        assert_eq!(b.notes.last().unwrap(), "payout failed; reward back on the board");
        as_(accounts(0), NearToken::from_yoctonear(0));
        assert_eq!(c.accept(0, None).status, BountyStatus::Paid);
    }

    #[test]
    fn successful_payout_stays_paid() {
        let mut c = accepted_board();
        callback(PromiseResult::Successful(vec![]));
        assert!(c.on_payout(0, true));
        assert_eq!(c.get_bounty(0).unwrap().status, BountyStatus::Paid);
    }

    #[test]
    fn failed_refund_reopens_bounty() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
        c.cancel(0);
        callback(PromiseResult::Failed);
        assert!(!c.on_payout(0, false));
        assert_eq!(c.get_bounty(0).unwrap().status, BountyStatus::Open);
    }

    #[test]
    fn reject_reopens() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
        as_(accounts(1), NearToken::from_yoctonear(0));
        c.submit_work(0, accounts(1), "sha256:ab".into());
        as_(accounts(0), NearToken::from_yoctonear(0));
        assert_eq!(c.reject_work(0, "tests fail".into()).status, BountyStatus::Open);
    }
}
