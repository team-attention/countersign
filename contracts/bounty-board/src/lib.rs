//! Bounty board: a client locks NEAR against a coding task, a worker submits a patch hash,
//! the client checks the work off-chain (runs the hidden tests) and accepts or rejects.
//! Accepting pays the reward to the worker's payout account (for the demo: the agent's
//! Countersign wallet), so the agent's earnings land under its spending policy.

use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::store::IterableMap;
use near_sdk::{AccountId, BorshStorageKey, NearToken, PanicOnDefault, Promise, env, near, require};

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

    /// Callable with a function-call key (no deposit). The caller is the worker;
    /// `payout_to` receives the reward if accepted.
    pub fn submit_work(&mut self, id: u64, payout_to: AccountId, submission: String) -> Bounty {
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

    /// Client accepts the submission and the reward is paid out.
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

    pub fn cancel(&mut self, id: u64) -> Bounty {
        let mut b = self.get(id);
        require!(env::predecessor_account_id() == b.client, "only the client can cancel");
        require!(b.status == BountyStatus::Open, "only open bounties can be cancelled");
        b.status = BountyStatus::Cancelled;
        self.bounties.insert(id, b.clone());
        emit("bounty_cancelled", json!(&b));
        Promise::new(b.client.clone())
            .transfer(NearToken::from_yoctonear(b.reward.0))
            .detach();
        b
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
        c.submit_work(0, accounts(2), "sha256:ab".into());
        as_(accounts(0), NearToken::from_yoctonear(0));
        let b = c.accept(0, None);
        assert_eq!(b.status, BountyStatus::Paid);
        assert_eq!(b.payout_to, Some(accounts(2)));
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
