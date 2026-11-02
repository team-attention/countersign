//! Watchlist: a blocklist that Countersign wallets share, built from rejections.
//!
//! One wallet's owner rejecting a payment says little. Several unrelated owners rejecting
//! payments to the same receiver says a lot. A member wallet reports a receiver when its owner
//! calls `reject_and_report`; the report carries the wallet's request id, so anyone can open
//! that wallet's `get_request` and check that the request exists and was rejected.
//!
//! - Only wallets the curator added (`add_member`) can report, one report per wallet per receiver.
//! - A receiver is `flagged` once `threshold` distinct wallets reported it.
//! - Wallets ask `check(receiver)` before deciding a payment: flagged -> blocked,
//!   reported but under the threshold -> held for the owner.
//! - The curator can `clear` a receiver (an appeal). Clearing is logged; reports are not edited.

use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{AccountId, BorshStorageKey, NearToken, PanicOnDefault, env, near, require};

const MAX_REASON_LEN: usize = 280;

#[near(serializers = [borsh])]
#[derive(BorshStorageKey)]
enum StorageKey {
    Members,
    Reports,
}

#[near(serializers = [json, borsh])]
#[derive(Clone, Debug)]
pub struct Report {
    /// The Countersign wallet that rejected the payment (the caller of `report`).
    pub wallet: AccountId,
    /// Request id in that wallet: `wallet.get_request({"id": request_id})` shows the rejection.
    pub request_id: u64,
    pub amount: U128,
    /// The agent's stated reason for the payment, as the wallet recorded it.
    pub reason: String,
    /// The owner's rejection note.
    pub note: String,
    pub reported_at_ns: u64,
}

#[near(serializers = [json])]
pub struct Check {
    pub receiver: AccountId,
    pub reports: u32,
    pub threshold: u32,
    pub flagged: bool,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct Watchlist {
    curator: AccountId,
    threshold: u32,
    members: IterableSet<AccountId>,
    reports: IterableMap<AccountId, Vec<Report>>,
}

fn emit(event: &str, data: near_sdk::serde_json::Value) {
    env::log_str(&format!(
        "EVENT_JSON:{}",
        json!({"standard": "countersign-watchlist", "version": "1.0.0", "event": event, "data": [data]})
    ));
}

fn trim(mut s: String) -> String {
    if s.len() > MAX_REASON_LEN {
        let mut cut = MAX_REASON_LEN;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

#[near]
impl Watchlist {
    #[init]
    pub fn new(curator: AccountId, threshold: u32) -> Self {
        require!(threshold >= 1, "threshold must be at least 1");
        Self {
            curator,
            threshold,
            members: IterableSet::new(StorageKey::Members),
            reports: IterableMap::new(StorageKey::Reports),
        }
    }

    // ---------------------------------------------------------------- member wallets

    /// Called by a member wallet contract when its owner rejects a payment and reports it.
    /// Returns the receiver's new count.
    pub fn report(
        &mut self,
        receiver: AccountId,
        request_id: u64,
        amount: U128,
        reason: String,
        note: String,
    ) -> Check {
        let wallet = env::predecessor_account_id();
        require!(self.members.contains(&wallet), "only member wallets can report");
        let mut list = self.reports.get(&receiver).cloned().unwrap_or_default();
        require!(
            !list.iter().any(|r| r.wallet == wallet),
            "this wallet already reported this receiver"
        );
        let report = Report {
            wallet,
            request_id,
            amount,
            reason: trim(reason),
            note: trim(note),
            reported_at_ns: env::block_timestamp(),
        };
        emit("reported", json!({"receiver": receiver, "report": report}));
        list.push(report);
        let was_flagged = (list.len() as u32 - 1) >= self.threshold;
        self.reports.insert(receiver.clone(), list);
        let c = self.check(receiver);
        if c.flagged && !was_flagged {
            emit("flagged", json!(&c));
        }
        c
    }

    // ---------------------------------------------------------------- curator

    #[payable]
    pub fn add_member(&mut self, wallet: AccountId) {
        self.assert_curator();
        self.members.insert(wallet.clone());
        emit("member_added", json!({"wallet": wallet}));
    }

    #[payable]
    pub fn remove_member(&mut self, wallet: AccountId) {
        self.assert_curator();
        self.members.remove(&wallet);
        emit("member_removed", json!({"wallet": wallet}));
    }

    /// Appeal path: drop every report for a receiver. Logged with the curator's note.
    #[payable]
    pub fn clear(&mut self, receiver: AccountId, note: String) {
        self.assert_curator();
        let removed = self.reports.remove(&receiver).map(|l| l.len()).unwrap_or(0);
        emit(
            "cleared",
            json!({"receiver": receiver, "reports_removed": removed, "note": trim(note)}),
        );
    }

    // ---------------------------------------------------------------- views

    pub fn check(&self, receiver: AccountId) -> Check {
        let reports = self.reports.get(&receiver).map(|l| l.len()).unwrap_or(0) as u32;
        Check {
            receiver,
            reports,
            threshold: self.threshold,
            flagged: reports >= self.threshold,
        }
    }

    pub fn get_reports(&self, receiver: AccountId) -> Vec<Report> {
        self.reports.get(&receiver).cloned().unwrap_or_default()
    }

    pub fn list_flagged(&self) -> Vec<Check> {
        self.reports
            .iter()
            .filter(|(_, l)| l.len() as u32 >= self.threshold)
            .map(|(r, _)| self.check(r.clone()))
            .collect()
    }

    pub fn list_members(&self) -> Vec<AccountId> {
        self.members.iter().cloned().collect()
    }

    pub fn get_config(&self) -> near_sdk::serde_json::Value {
        json!({"curator": self.curator, "threshold": self.threshold, "members": self.members.len()})
    }

    fn assert_curator(&self) {
        require!(
            env::predecessor_account_id() == self.curator,
            "only the curator can do this"
        );
        require!(
            env::attached_deposit() == NearToken::from_yoctonear(1),
            "attach exactly 1 yoctoNEAR (full-access key required)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::{VMContextBuilder, accounts};
    use near_sdk::testing_env;

    fn as_(who: AccountId, deposit: u128) {
        let mut b = VMContextBuilder::new();
        b.current_account_id("watchlist.near".parse().unwrap())
            .predecessor_account_id(who)
            .attached_deposit(NearToken::from_yoctonear(deposit));
        testing_env!(b.build());
    }

    fn setup() -> Watchlist {
        as_(accounts(0), 0);
        let mut w = Watchlist::new(accounts(0), 2);
        as_(accounts(0), 1);
        w.add_member(accounts(1));
        w.add_member(accounts(2));
        w
    }

    fn report(w: &mut Watchlist, wallet: AccountId, id: u64) -> Check {
        as_(wallet, 0);
        w.report(accounts(4), id, U128(5), "license fee".into(), "injected".into())
    }

    #[test]
    fn flagged_only_after_threshold_distinct_wallets() {
        let mut w = setup();
        let c = report(&mut w, accounts(1), 3);
        assert_eq!((c.reports, c.flagged), (1, false));
        let c = report(&mut w, accounts(2), 0);
        assert_eq!((c.reports, c.flagged), (2, true));
        assert_eq!(w.list_flagged().len(), 1);
        assert_eq!(w.get_reports(accounts(4))[0].request_id, 3);
    }

    #[test]
    #[should_panic(expected = "already reported")]
    fn one_report_per_wallet() {
        let mut w = setup();
        report(&mut w, accounts(1), 0);
        report(&mut w, accounts(1), 1);
    }

    #[test]
    #[should_panic(expected = "only member wallets")]
    fn non_member_cannot_report() {
        let mut w = setup();
        report(&mut w, accounts(3), 0);
    }

    #[test]
    #[should_panic(expected = "attach exactly 1 yoctoNEAR")]
    fn curator_needs_full_access_key() {
        let mut w = setup();
        as_(accounts(0), 0);
        w.add_member(accounts(3));
    }

    #[test]
    fn clear_drops_reports() {
        let mut w = setup();
        report(&mut w, accounts(1), 0);
        report(&mut w, accounts(2), 0);
        as_(accounts(0), 1);
        w.clear(accounts(4), "appeal accepted".into());
        assert!(!w.check(accounts(4)).flagged);
        assert!(w.list_flagged().is_empty());
    }
}
