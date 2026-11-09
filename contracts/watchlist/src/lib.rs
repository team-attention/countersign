//! Watchlist: a blocklist that Countersign wallets share, built from rejections.
//!
//! One wallet's owner rejecting a payment says little. Several unrelated owners rejecting
//! payments to the same receiver says a lot. A member wallet reports a receiver when its owner
//! calls `reject_and_report`; the report carries the wallet's request id. Before the report
//! counts, the watchlist calls that wallet's `get_request` itself and records it only if the
//! request exists, was rejected, and names the same receiver and amount.
//!
//! - Only wallets the curator added (`add_member`) can report, one report per wallet per receiver.
//! - Reason and note are copied from the wallet's request, not from the caller's arguments.
//! - A receiver is `flagged` once `threshold` distinct wallets reported it.
//! - Wallets ask `check(receiver)` before deciding a payment: flagged -> blocked,
//!   reported but under the threshold -> held for the owner.
//! - The curator can `clear` a receiver (an appeal). Clearing is logged; reports are not edited.
//!   After a clear, only rejections the wallet decided later count: a report still in flight
//!   during the clear, or an old rejection sent again, is refused.

use near_sdk::json_types::U128;
use near_sdk::serde_json::json;
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{
    AccountId, BorshStorageKey, Gas, NearToken, PanicOnDefault, Promise, PromiseError, env, ext_contract, near,
    require,
};

const MAX_REASON_LEN: usize = 280;
const GET_REQUEST_GAS: Gas = Gas::from_tgas(5);
const VERIFY_GAS: Gas = Gas::from_tgas(10);

/// The part of a Countersign wallet (`contracts/countersign`) the watchlist reads.
#[ext_contract(ext_wallet)]
#[allow(dead_code)]
trait Wallet {
    fn get_request(&self, id: u64) -> Option<WalletRequest>;
}

/// The fields of a Countersign `PaymentRequest` the evidence check needs.
#[near(serializers = [json])]
pub struct WalletRequest {
    pub receiver: AccountId,
    pub amount: U128,
    pub reason: String,
    pub status: String,
    pub notes: Vec<String>,
    pub decided_at_ns: Option<u64>,
}

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

/// Raw key (outside the state struct, so deployed state stays readable) holding the block time
/// of the last `clear` for a receiver.
fn cleared_key(receiver: &AccountId) -> Vec<u8> {
    [b"cleared_at:".as_slice(), receiver.as_bytes()].concat()
}

fn cleared_at(receiver: &AccountId) -> Option<u64> {
    env::storage_read(&cleared_key(receiver)).map(|v| u64::from_le_bytes(v.try_into().unwrap()))
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
    /// The report counts only after `on_report_verified` read the request back from the wallet.
    /// `reason` and `note` are ignored in favour of what the wallet recorded (kept for the
    /// Countersign call signature).
    #[allow(unused_variables)]
    pub fn report(
        &mut self,
        receiver: AccountId,
        request_id: u64,
        amount: U128,
        reason: String,
        note: String,
    ) -> Promise {
        let wallet = env::predecessor_account_id();
        self.assert_can_report(&wallet, &receiver);
        ext_wallet::ext(wallet.clone())
            .with_static_gas(GET_REQUEST_GAS)
            .get_request(request_id)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(VERIFY_GAS)
                    .on_report_verified(receiver, wallet, request_id, amount),
            )
    }

    /// Records the report if the wallet's own `get_request` backs it up. Returns the receiver's
    /// new count; panics (and records nothing) if the evidence does not match.
    #[private]
    pub fn on_report_verified(
        &mut self,
        receiver: AccountId,
        wallet: AccountId,
        request_id: u64,
        amount: U128,
        #[callback_result] evidence: Result<Option<WalletRequest>, PromiseError>,
    ) -> Check {
        let req = match evidence {
            Ok(Some(req)) => req,
            Ok(None) => env::panic_str("evidence check failed: no such request in the wallet"),
            Err(_) => env::panic_str("evidence check failed: the wallet has no readable get_request"),
        };
        require!(req.status == "Rejected", "evidence check failed: the request was not rejected");
        require!(req.receiver == receiver, "evidence check failed: the request pays a different receiver");
        require!(req.amount == amount, "evidence check failed: the amount does not match");
        if let Some(t) = cleared_at(&receiver) {
            require!(
                req.decided_at_ns.is_some_and(|d| d > t),
                "evidence check failed: the rejection was decided before the curator's last clear"
            );
        }
        // membership or a duplicate may have changed while the check was in flight
        self.assert_can_report(&wallet, &receiver);
        let mut list = self.reports.get(&receiver).cloned().unwrap_or_default();
        let report = Report {
            wallet,
            request_id,
            amount,
            reason: trim(req.reason),
            note: trim(req.notes.last().cloned().unwrap_or_default()),
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

    fn assert_can_report(&self, wallet: &AccountId, receiver: &AccountId) {
        require!(self.members.contains(wallet), "only member wallets can report");
        require!(
            !self
                .reports
                .get(receiver)
                .is_some_and(|l| l.iter().any(|r| &r.wallet == wallet)),
            "this wallet already reported this receiver"
        );
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
        env::storage_write(&cleared_key(&receiver), &env::block_timestamp().to_le_bytes());
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
        as_at(who, deposit, 10);
    }

    fn as_at(who: AccountId, deposit: u128, now_ns: u64) {
        let mut b = VMContextBuilder::new();
        b.current_account_id("watchlist.near".parse().unwrap())
            .predecessor_account_id(who)
            .block_timestamp(now_ns)
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

    fn rejected(status: &str, receiver: AccountId, amount: u128) -> Option<WalletRequest> {
        Some(WalletRequest {
            receiver,
            amount: U128(amount),
            reason: "license fee".into(),
            status: status.into(),
            notes: vec!["injected".into()],
            decided_at_ns: Some(5),
        })
    }

    /// `report` followed by the callback, with the wallet's `get_request` answer given directly.
    fn report_with(w: &mut Watchlist, wallet: AccountId, id: u64, evidence: Option<WalletRequest>) -> Check {
        as_(wallet.clone(), 0);
        let _ = w.report(accounts(4), id, U128(5), "caller text".into(), "caller text".into());
        as_("watchlist.near".parse().unwrap(), 0);
        w.on_report_verified(accounts(4), wallet, id, U128(5), Ok(evidence))
    }

    fn report(w: &mut Watchlist, wallet: AccountId, id: u64) -> Check {
        report_with(w, wallet, id, rejected("Rejected", accounts(4), 5))
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
    fn reason_and_note_come_from_the_wallet() {
        let mut w = setup();
        report(&mut w, accounts(1), 0);
        let r = &w.get_reports(accounts(4))[0];
        assert_eq!((r.reason.as_str(), r.note.as_str()), ("license fee", "injected"));
    }

    #[test]
    #[should_panic(expected = "was not rejected")]
    fn evidence_must_be_rejected() {
        let mut w = setup();
        report_with(&mut w, accounts(1), 0, rejected("AutoPaid", accounts(4), 5));
    }

    #[test]
    #[should_panic(expected = "different receiver")]
    fn evidence_must_name_the_receiver() {
        let mut w = setup();
        report_with(&mut w, accounts(1), 0, rejected("Rejected", accounts(5), 5));
    }

    #[test]
    #[should_panic(expected = "amount does not match")]
    fn evidence_must_match_the_amount() {
        let mut w = setup();
        report_with(&mut w, accounts(1), 0, rejected("Rejected", accounts(4), 6));
    }

    #[test]
    #[should_panic(expected = "no such request")]
    fn evidence_must_exist() {
        let mut w = setup();
        report_with(&mut w, accounts(1), 9, None);
    }

    #[test]
    #[should_panic(expected = "no readable get_request")]
    fn member_that_is_not_a_wallet_cannot_report() {
        let mut w = setup();
        as_(accounts(1), 0);
        let _ = w.report(accounts(4), 0, U128(5), "x".into(), "x".into());
        as_("watchlist.near".parse().unwrap(), 0);
        w.on_report_verified(accounts(4), accounts(1), 0, U128(5), Err(PromiseError::Failed));
    }

    #[test]
    #[should_panic(expected = "only member wallets")]
    fn member_removed_during_the_check_is_not_counted() {
        let mut w = setup();
        as_(accounts(1), 0);
        let _ = w.report(accounts(4), 0, U128(5), "x".into(), "x".into());
        as_(accounts(0), 1);
        w.remove_member(accounts(1));
        as_("watchlist.near".parse().unwrap(), 0);
        w.on_report_verified(accounts(4), accounts(1), 0, U128(5), Ok(rejected("Rejected", accounts(4), 5)));
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

    #[test]
    #[should_panic(expected = "before the curator's last clear")]
    fn report_in_flight_during_a_clear_is_not_counted() {
        let mut w = setup();
        as_(accounts(1), 0);
        let _ = w.report(accounts(4), 0, U128(5), "x".into(), "x".into());
        as_(accounts(0), 1);
        w.clear(accounts(4), "appeal accepted".into());
        as_("watchlist.near".parse().unwrap(), 0);
        w.on_report_verified(accounts(4), accounts(1), 0, U128(5), Ok(rejected("Rejected", accounts(4), 5)));
    }

    #[test]
    #[should_panic(expected = "before the curator's last clear")]
    fn old_rejection_cannot_undo_a_clear() {
        let mut w = setup();
        report(&mut w, accounts(1), 0);
        as_(accounts(0), 1);
        w.clear(accounts(4), "appeal accepted".into());
        report(&mut w, accounts(1), 0);
    }

    #[test]
    fn rejection_decided_after_a_clear_counts() {
        let mut w = setup();
        report(&mut w, accounts(1), 0);
        as_(accounts(0), 1);
        w.clear(accounts(4), "appeal accepted".into());
        let mut later = rejected("Rejected", accounts(4), 5);
        later.as_mut().unwrap().decided_at_ns = Some(20);
        as_at(accounts(1), 0, 30);
        let _ = w.report(accounts(4), 1, U128(5), "x".into(), "x".into());
        as_at("watchlist.near".parse().unwrap(), 0, 30);
        let c = w.on_report_verified(accounts(4), accounts(1), 1, U128(5), Ok(later));
        assert_eq!((c.reports, c.flagged), (1, false));
    }
}
