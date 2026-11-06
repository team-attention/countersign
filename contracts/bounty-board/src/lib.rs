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
const MAX_NOTE: usize = 200;
const MAX_NOTES: usize = 10;
/// Storage a bounty can occupy beyond its title, spec and client: record and index
/// overhead, then the worst case of what later calls add (worker, payout_to, submission,
/// MAX_NOTES notes of MAX_NOTE bytes). Unit test `storage_fee_covers_worst_case` checks it.
const BASE_BYTES: u64 = 400;
const GROWTH_BYTES: u64 = 2 * (1 + 4 + 64) + (1 + 4 + 200) + (MAX_NOTES * (4 + MAX_NOTE)) as u64;
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

    /// Lock the attached deposit as the reward, minus `storage_fee`. The fee stays on the
    /// board for good: a cancelled bounty keeps its record, so posting and cancelling in a
    /// loop costs the poster, not the board.
    #[payable]
    pub fn post_bounty(&mut self, title: String, spec: String) -> Bounty {
        require!(title.len() <= 120 && spec.len() <= MAX_TEXT, "text too long");
        let fee = self.storage_fee(env::predecessor_account_id(), title.clone(), spec.clone()).0;
        let reward = env::attached_deposit().as_yoctonear().saturating_sub(fee);
        require!(
            reward > NearToken::from_millinear(10).as_yoctonear(),
            "reward after the storage fee must be over 0.01 NEAR"
        );
        let b = Bounty {
            id: self.next_id,
            client: env::predecessor_account_id(),
            title,
            spec,
            reward: U128(reward),
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
        add_note(&mut b, note.unwrap_or_else(|| "accepted".into()));
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
        add_note(&mut b, format!("rejected: {note}"));
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
                add_note(&mut b, "payout failed; reward back on the board".into());
            } else {
                b.status = BountyStatus::Open;
                add_note(&mut b, "refund failed; bounty reopened".into());
            }
            self.bounties.insert(id, b.clone());
            emit("payout_failed", json!(&b));
        }
        ok
    }

    /// What `post_bounty` keeps from the deposit to pay for the bounty's storage.
    pub fn storage_fee(&self, client: AccountId, title: String, spec: String) -> U128 {
        let bytes = BASE_BYTES + GROWTH_BYTES + (client.len() + title.len() + spec.len()) as u64;
        U128(env::storage_byte_cost().as_yoctonear() * bytes as u128)
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

/// Notes are capped at MAX_NOTE bytes each and MAX_NOTES per bounty (the newest replaces the
/// last one), so a client cannot grow a record past what `storage_fee` paid for.
fn add_note(b: &mut Bounty, note: String) {
    require!(note.len() <= MAX_NOTE, "note too long");
    if b.notes.len() >= MAX_NOTES {
        b.notes.pop();
    }
    b.notes.push(note);
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

    #[test]
    fn storage_fee_covers_worst_case() {
        let client: AccountId = format!("{}.testnet", "c".repeat(56)).parse().unwrap();
        let worker: AccountId = format!("{}.testnet", "w".repeat(56)).parse().unwrap();
        as_(client.clone(), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        let (title, spec) = ("t".repeat(120), "s".repeat(MAX_TEXT));
        let fee = c.storage_fee(client.clone(), title.clone(), spec.clone()).0;
        let before = env::storage_usage();
        c.post_bounty(title, spec);
        for i in 0..MAX_NOTES + 3 {
            as_(worker.clone(), NearToken::from_yoctonear(0));
            c.submit_work(0, worker.clone(), "x".repeat(200));
            as_(client.clone(), NearToken::from_yoctonear(0));
            if i < MAX_NOTES + 2 {
                c.reject_work(0, "n".repeat(MAX_NOTE - 10));
            } else {
                c.accept(0, Some("a".repeat(MAX_NOTE)));
            }
        }
        c.bounties.flush();
        let used = env::storage_usage() - before;
        assert_eq!(c.get_bounty(0).unwrap().notes.len(), MAX_NOTES);
        assert!(
            used as u128 * env::storage_byte_cost().as_yoctonear() <= fee,
            "used {used} bytes, fee covers {}",
            fee / env::storage_byte_cost().as_yoctonear()
        );
    }

    #[test]
    fn fee_is_kept_from_the_deposit() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        let fee = c.storage_fee(accounts(0), "fix it".into(), "tasks/x".into()).0;
        let b = c.post_bounty("fix it".into(), "tasks/x".into());
        assert_eq!(b.reward.0, NearToken::from_near(1).as_yoctonear() - fee);
    }

    #[test]
    #[should_panic(expected = "reward after the storage fee must be over 0.01 NEAR")]
    fn deposit_that_only_covers_storage_is_refused() {
        as_(accounts(0), NearToken::from_millinear(30));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
    }

    #[test]
    #[should_panic(expected = "note too long")]
    fn long_note_is_refused() {
        as_(accounts(0), NearToken::from_near(1));
        let mut c = BountyBoard::new();
        c.post_bounty("fix it".into(), "tasks/x".into());
        as_(accounts(1), NearToken::from_yoctonear(0));
        c.submit_work(0, accounts(1), "sha256:ab".into());
        as_(accounts(0), NearToken::from_yoctonear(0));
        c.accept(0, Some("a".repeat(MAX_NOTE + 1)));
    }
}
