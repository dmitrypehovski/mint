//! Approvals: who authorised one action, proven rather than asserted.
//!
//! A gate that holds writes is only as good as its answer to one question: who said yes. The first
//! version of this runtime answered it with a string — `approve:<identity>` — which anybody who could
//! type it could produce. The rules were right and the authorisation was worthless: an approval that
//! cannot name its issuer is a comment, not a permission.
//!
//! This module gives an approval an issuer, and checks it the way the capability work in this field
//! checks tokens. In the order they are applied, and none of them skippable:
//!
//! 1. **issuer pinning** — the signature's key must be one the runtime was told to trust;
//! 2. **signature** — over the exact bytes of the approval;
//! 3. **identifier binding** — it must name this action, not actions in general;
//! 4. **time bounds** — it must be valid now, which means it can also stop being valid;
//! 5. **agent scope** — it must name the runtime it was issued for, so an approval for one run is not
//!    usable by another;
//! 6. **single use** — enforced by the gate, which releases a given action once and records that the
//!    second attempt was a refusal;
//! 7. **spec** — an object this runtime does not recognise is refused rather than read optimistically.
//!
//! What is deliberately absent, and named so that nobody assumes it: no revocation list, no delegation
//! chain (an approval cannot be re-delegated, which is a limitation rather than a feature), and no
//! counter-signature by the runtime.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::receipt::{verify_signature, Sealer};

pub const SPEC: &str = "mint.approval/0.1";

/// What an approval asserts. Signed as a whole.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalBody {
    pub spec: String,
    /// The exact action this authorises: the same identity string the gate computes for a step.
    pub action: String,
    /// Which runtime may use it. An approval is not transferable between agents.
    pub for_agent: String,
    pub issued_at_ms: u128,
    /// When it stops being usable. An approval with no end is an approval nobody can take back.
    pub not_after_ms: u128,
    /// Why it was given, in the issuer's words. Optional, and carried so a reader of the log can see
    /// the reason rather than only the fact.
    #[serde(default)]
    pub note: String,
    /// Whether a person was at a terminal when this was issued. `"terminal"`, or `"not-a-terminal"` for
    /// an approval produced by a script, a CI job, or an agent that ran the command itself.
    ///
    /// This is a record, not a defence, and saying so is the point. A key that an agent can read is a key
    /// an agent can use, so nothing here can stop an approval being issued on the wrong side of the line.
    /// What it can do is make the difference visible: the log says how each approval came to exist, and a
    /// person reading it can see that the approval for a deletion was issued by a script at 04:00.
    /// Appended last, and absent when empty, so approvals issued before this field existed still verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_from: Option<String>,
}

/// Whether a person was at a terminal, described for the record rather than enforced.
pub fn issued_from() -> String {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        "terminal".to_string()
    } else {
        "not-a-terminal".to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Approval {
    pub body: ApprovalBody,
    pub sig: crate::endorse::Sig,
}

impl Approval {
    /// The bytes that were signed: the body, serialized in declaration order.
    pub fn canonical(&self) -> String {
        serde_json::to_string(&self.body).unwrap_or_default()
    }

    /// Sign an approval. Run by whoever is answering for the action, with their own key.
    pub fn issue(
        sealer: &Sealer,
        action: &str,
        for_agent: &str,
        issued_at_ms: u128,
        not_after_ms: u128,
        note: &str,
    ) -> Approval {
        let body = ApprovalBody {
            // Recorded always, so "no terminal" and "before this field existed" are different things.
            issued_from: Some(issued_from()),
            spec: SPEC.to_string(),
            action: action.to_string(),
            for_agent: for_agent.to_string(),
            issued_at_ms,
            not_after_ms,
            note: note.to_string(),
        };
        let canonical = serde_json::to_string(&body).unwrap_or_default();
        let sig = sealer.sign_bytes(canonical.as_bytes());
        Approval {
            body,
            sig: crate::endorse::Sig {
                alg: "ed25519".into(),
                kid: sealer.agent.clone(),
                value: sig.iter().map(|b| format!("{b:02x}")).collect(),
            },
        }
    }
}

/// The approvers a runtime was told to trust, by key, with the names to record beside them.
#[derive(Clone, Debug, Default)]
pub struct Approvers {
    entries: Vec<(String, String)>,
    /// Set when a pinned set was asked for and could not be read. It is not the same as an empty set:
    /// an empty set means nobody must sign, and this means only a signature will do. Falling back to
    /// "nobody must sign" because a file was missing is how a pin quietly stops pinning.
    require_signed: bool,
}

impl Approvers {
    pub fn empty() -> Approvers {
        Approvers { entries: Vec::new(), require_signed: false }
    }

    /// A set that could not be read. Nothing is trusted to sign, and a bare token is refused as
    /// firmly as a stranger's signature.
    pub fn unreadable() -> Approvers {
        Approvers { entries: Vec::new(), require_signed: true }
    }

    /// Whether an unsigned token still counts. Only where nothing was pinned at all: the rules
    /// demonstration, where a visitor's switch stands in for a person. Anywhere else, saying yes
    /// requires a signature from somebody the runtime was told to trust.
    pub fn allows_tokens(&self) -> bool {
        !self.require_signed && self.entries.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Read the pinned set from a file: a list of keys, or a list of objects with `key` and `name`.
    /// Both are accepted because the second is what a person will write and the first is what a tool
    /// will emit.
    pub fn from_file(path: &str) -> Result<Approvers> {
        let text = std::fs::read_to_string(path)?;
        let value: serde_json::Value = serde_json::from_str(&text)?;
        let list = value
            .get("approvers")
            .and_then(|a| a.as_array())
            .or_else(|| value.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "expected a list of approvers, or an object with an \"approvers\" list"
                )
            })?;
        let mut entries = Vec::new();
        for item in list {
            match item {
                serde_json::Value::String(key) => {
                    entries.push((key.trim().to_lowercase(), String::new()))
                }
                serde_json::Value::Object(_) => {
                    let key = item
                        .get("key")
                        .and_then(|k| k.as_str())
                        .ok_or_else(|| anyhow::anyhow!("an approver entry has no \"key\""))?;
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string();
                    entries.push((key.trim().to_lowercase(), name));
                }
                _ => return Err(anyhow::anyhow!("an approver entry is neither a key nor an object")),
            }
        }
        Ok(Approvers { entries, require_signed: true })
    }

    pub fn contains(&self, key: &str) -> bool {
        let k = key.trim().to_lowercase();
        self.entries.iter().any(|(hex, _)| *hex == k)
    }

    /// The name to record for a key: the pinned name, or the key itself shortened, so an approval is
    /// never recorded against a nameless blank.
    pub fn name_of(&self, key: &str) -> String {
        let k = key.trim().to_lowercase();
        self.entries
            .iter()
            .find(|(hex, _)| *hex == k)
            .map(|(_, name)| {
                if name.is_empty() {
                    k.chars().take(12).collect::<String>()
                } else {
                    name.clone()
                }
            })
            .unwrap_or_else(|| k.chars().take(12).collect::<String>())
    }

    /// A line describing the pinned set, for a run to print before it relies on it.
    pub fn describe(&self) -> String {
        if self.entries.is_empty() {
            return "none pinned".to_string();
        }
        self.entries
            .iter()
            .map(|(hex, name)| {
                if name.is_empty() {
                    hex.chars().take(12).collect::<String>()
                } else {
                    format!("{name} ({})", hex.chars().take(12).collect::<String>())
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What a presented approval turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Accepted { approver: String, name: String },
    Refused { why: String },
}

impl Check {
    pub fn accepted(&self) -> Option<(String, String)> {
        match self {
            Check::Accepted { approver, name } => Some((approver.clone(), name.clone())),
            Check::Refused { .. } => None,
        }
    }
}

/// Check an approval against everything it has to be true of, and say which check it failed.
pub fn check(
    approval: &Approval,
    pinned: &Approvers,
    action: &str,
    agent: &str,
    now_ms: u128,
) -> Check {
    let refuse = |why: String| Check::Refused { why };

    if approval.body.spec != SPEC {
        return refuse(format!(
            "this is a «{}», which this runtime does not know how to read",
            approval.body.spec
        ));
    }
    // 1. the issuer must be somebody this runtime was told to trust. Checked before the signature
    //    because a valid signature from a stranger is exactly the thing pinning exists to refuse.
    if !pinned.contains(&approval.sig.kid) {
        return refuse(format!(
            "signed by {}, who is not one of the approvers this runtime pins ({})",
            short(&approval.sig.kid),
            pinned.describe()
        ));
    }
    // 2. the signature, over the bytes of the body.
    if let Err(e) = verify_signature(&approval.sig.kid, approval.canonical().as_bytes(), &approval.sig.value) {
        return refuse(format!("the signature does not hold: {e}"));
    }
    if approval.sig.alg != "ed25519" {
        return refuse(format!(
            "the signature names the algorithm «{}», which this runtime does not know",
            approval.sig.alg
        ));
    }
    // 3. it must name this action and no other.
    if approval.body.action != action {
        return refuse(format!(
            "it authorises «{}», and the action waiting is «{}»",
            short(&approval.body.action),
            short(action)
        ));
    }
    // 4. it must be valid now, which is what makes it possible to stop being valid.
    if now_ms < approval.body.issued_at_ms {
        return refuse(format!(
            "it is not valid until {} and the clock says {now_ms}",
            approval.body.issued_at_ms
        ));
    }
    if now_ms > approval.body.not_after_ms {
        return refuse(format!(
            "it expired at {} and the clock says {now_ms}",
            approval.body.not_after_ms
        ));
    }
    // 5. it must name this runtime: an approval issued for one agent is not usable by another.
    if approval.body.for_agent != agent {
        return refuse(format!(
            "it was issued for the runtime {}, and this one is {}",
            short(&approval.body.for_agent),
            short(agent)
        ));
    }

    Check::Accepted {
        approver: approval.sig.kid.clone(),
        name: pinned.name_of(&approval.sig.kid),
    }
}

fn short(s: &str) -> String {
    s.chars().take(20).collect()
}


// ── withdrawal ───────────────────────────────────────────────────────────────────────────────────

pub const REVOCATION_SPEC: &str = "mint.revocation/0.1";

/// A withdrawal: this approval, or every approval issued before this moment, no longer authorises
/// anything.
///
/// An approval with an expiry can be waited out, which is no help at all when the reason for taking it
/// back is that something changed. The alternative to a withdrawal is a person watching a clock, so it
/// is a signed object like everything else here: pinned issuer, signature, and a time that says which
/// approvals it reaches. Approvals issued *after* it are untouched, because a withdrawal is not a
/// standing prohibition — it is the answer to an approval already given.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RevocationBody {
    pub spec: String,
    /// The action whose approval is withdrawn, or `*` for every approval issued before `at_ms`.
    pub action: String,
    pub at_ms: u128,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Revocation {
    pub body: RevocationBody,
    pub sig: crate::endorse::Sig,
}

impl Revocation {
    pub fn canonical(&self) -> String {
        serde_json::to_string(&self.body).unwrap_or_default()
    }

    pub fn issue(sealer: &Sealer, action: &str, at_ms: u128, note: &str) -> Revocation {
        let body = RevocationBody {
            spec: REVOCATION_SPEC.to_string(),
            action: action.to_string(),
            at_ms,
            note: note.to_string(),
        };
        let canonical = serde_json::to_string(&body).unwrap_or_default();
        let sig = sealer.sign_bytes(canonical.as_bytes());
        Revocation {
            body,
            sig: crate::endorse::Sig {
                alg: "ed25519".into(),
                kid: sealer.agent.clone(),
                value: sig.iter().map(|b| format!("{b:02x}")).collect(),
            },
        }
    }
}

/// Whether any of these withdrawals reaches an approval for this action issued at `approved_at_ms`.
///
/// A withdrawal from a key the runtime does not pin is ignored rather than obeyed, and so is one whose
/// signature does not hold: otherwise anybody could take back anybody's approval, which is a worse
/// failure than the one this exists to fix. The reason is returned rather than a boolean, because a
/// person reading the log needs to know who withdrew what.
pub fn revoked(
    revocations: &[Revocation],
    pinned: &Approvers,
    action: &str,
    approved_at_ms: u128,
    now_ms: u128,
) -> Option<String> {
    for r in revocations {
        if r.body.spec != REVOCATION_SPEC {
            continue;
        }
        if !pinned.contains(&r.sig.kid) {
            continue;
        }
        if verify_signature(&r.sig.kid, r.canonical().as_bytes(), &r.sig.value).is_err() {
            continue;
        }
        if r.body.at_ms > now_ms {
            continue; // dated in the future: not in force yet
        }
        if r.body.action != action && r.body.action != "*" {
            continue;
        }
        // Only approvals issued at or before the withdrawal are reached.
        if approved_at_ms > r.body.at_ms {
            continue;
        }
        let who = pinned.name_of(&r.sig.kid);
        return Some(match r.body.action.as_str() {
            "*" => format!(
                "{who} withdrew every approval issued before {} — this one was issued at {approved_at_ms}",
                r.body.at_ms
            ),
            _ => format!("{who} withdrew the approval for this action at {}", r.body.at_ms),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTION: &str = "move:/invoices/a.pdf->2026-03/";
    const FAR: u128 = 4_102_444_800_000; // 2100

    /// A unique path per call. Tests run in parallel, and a fixed path is a race: one test writing
    /// while another reads gives an intermittent failure with nothing to do with the code under test.
    fn unique(tag: &str) -> String {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        format!("/tmp/mint-approval-{tag}-{}-{n}", std::process::id())
    }

    fn key(tag: &str) -> Sealer {
        let path = unique(tag);
        let _ = std::fs::remove_file(&path);
        Sealer::load_or_create(&path).expect("a key")
    }

    // One file per test: they run in parallel, and a shared path means one test reading another's set.
    fn pinned(tag: &str, keys: &[&Sealer]) -> Approvers {
        let entries = keys
            .iter()
            .map(|s| serde_json::json!({ "key": s.agent, "name": "dmitry" }))
            .collect::<Vec<_>>();
        let text = serde_json::to_string(&serde_json::json!({ "approvers": entries })).unwrap();
        let path = unique(&format!("pinned-{tag}"));
        std::fs::write(&path, text).unwrap();
        Approvers::from_file(&path).expect("a pinned set")
    }

    #[test]
    fn a_set_that_could_not_be_read_refuses_rather_than_falls_back() {
        assert!(Approvers::empty().allows_tokens(), "nothing pinned means the demonstration mode");
        assert!(!Approvers::unreadable().allows_tokens(), "a failed load must not allow tokens");
        assert!(!pinned("requires", &[&key("requires-issuer")]).allows_tokens());
    }

    #[test]
    fn an_approval_from_a_key_that_is_not_pinned_is_refused() {
        let issuer = key("unpinned-issuer");
        let other = key("pinned-other");
        let a = Approval::issue(&issuer, ACTION, "runtime-1", 0, FAR, "looks fine to me");
        match check(&a, &pinned("unpinned", &[&other]), ACTION, "runtime-1", 1) {
            Check::Refused { why } => assert!(why.contains("not one of the approvers"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_approval_for_another_action_is_refused() {
        let issuer = key("action-issuer");
        let a = Approval::issue(&issuer, "move:/other.pdf->2026-01/", "runtime-1", 0, FAR, "");
        match check(&a, &pinned("one-issuer", &[&issuer]), ACTION, "runtime-1", 1) {
            Check::Refused { why } => assert!(why.contains("the action waiting is"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_expired_approval_is_refused_even_though_everything_else_holds() {
        let issuer = key("expiry-issuer");
        let a = Approval::issue(&issuer, ACTION, "runtime-1", 0, 1_000, "");
        assert!(check(&a, &pinned("one-issuer", &[&issuer]), ACTION, "runtime-1", 500).accepted().is_some());
        match check(&a, &pinned("one-issuer", &[&issuer]), ACTION, "runtime-1", 5_000) {
            Check::Refused { why } => assert!(why.contains("it expired"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_approval_issued_for_another_runtime_is_refused() {
        let issuer = key("scope-issuer");
        let a = Approval::issue(&issuer, ACTION, "runtime-1", 0, FAR, "");
        match check(&a, &pinned("one-issuer", &[&issuer]), ACTION, "runtime-2", 1) {
            Check::Refused { why } => assert!(why.contains("issued for the runtime"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_edited_approval_does_not_hold() {
        let issuer = key("edited-issuer");
        let mut a = Approval::issue(&issuer, ACTION, "runtime-1", 0, FAR, "");
        a.body.not_after_ms = FAR * 2; // an expiry far in the future, added after signing
        match check(&a, &pinned("one-issuer", &[&issuer]), ACTION, "runtime-1", 1) {
            Check::Refused { why } => assert!(why.contains("the signature does not hold"), "{why}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_good_approval_is_accepted_and_names_its_issuer() {
        let issuer = key("good-issuer");
        let a = Approval::issue(&issuer, ACTION, "runtime-1", 0, FAR, "checked the invoice");
        let (approver, name) = check(&a, &pinned("one-issuer", &[&issuer]), ACTION, "runtime-1", 1)
            .accepted()
            .expect("accepted");
        assert_eq!(approver, issuer.agent);
        assert_eq!(name, "dmitry");
    }

    #[test]
    fn a_pinned_set_keeps_the_names_and_survives_round_tripping() {
        let issuer = key("set-issuer");
        let set = pinned("set", &[&issuer]);
        assert!(set.contains(&issuer.agent.to_uppercase()), "keys are compared without case");
        assert_eq!(set.name_of(&issuer.agent), "dmitry");
        assert_eq!(set.name_of("nobody"), "nobody", "an unknown key is named by itself");
        assert!(!set.is_empty());
    }
}

#[cfg(test)]
mod revocation_tests {
    use super::*;

    const ACTION: &str = "guard:aaa";
    const NOW: u128 = 1_000;

    fn key(tag: &str) -> Sealer {
        let path = format!("/tmp/mint-revoke-{tag}-{}", std::process::id());
        let _ = std::fs::remove_file(&path);
        Sealer::load_or_create(&path).expect("a key")
    }

    fn pinned(tag: &str, keys: &[&Sealer]) -> Approvers {
        let entries = keys
            .iter()
            .map(|s| serde_json::json!({ "key": s.agent, "name": "dmitry" }))
            .collect::<Vec<_>>();
        let path = format!("/tmp/mint-revoke-set-{tag}-{}.json", std::process::id());
        std::fs::write(&path, serde_json::to_string(&serde_json::json!({ "approvers": entries })).unwrap())
            .unwrap();
        Approvers::from_file(&path).expect("a pinned set")
    }

    #[test]
    fn an_approval_records_whether_a_person_was_at_a_terminal() {
        // A record, not a defence: it cannot stop an approval being issued by a script, and it means a
        // reader of the log can see that it was.
        let issuer = key("issued-from");
        let a = Approval::issue(&issuer, ACTION, &"a".repeat(64), 1, 2, "");
        assert!(matches!(
            a.body.issued_from.as_deref(),
            Some("terminal") | Some("not-a-terminal")
        ));
        // And it is inside the signature: editing it invalidates the approval like any other field.
        let set = pinned("issued-from", &[&issuer]);
        let mut edited = a.clone();
        edited.body.issued_from = Some("terminal".into());
        assert!(matches!(
            check(&edited, &set, ACTION, &"a".repeat(64), 5),
            Check::Refused { .. }
        ));
    }

    #[test]
    fn a_withdrawal_by_a_pinned_approver_reaches_the_approval() {
        let issuer = key("reaches");
        let set = pinned("reaches", &[&issuer]);
        let r = Revocation::issue(&issuer, ACTION, NOW, "the invoice turned out to be wrong");
        let why = revoked(&[r], &set, ACTION, 900, NOW + 1).expect("withdrawn");
        assert!(why.contains("dmitry withdrew"), "{why}");
    }

    #[test]
    fn a_withdrawal_from_a_key_nobody_pins_is_ignored() {
        // Otherwise anyone who can write a file could take back anyone's approval.
        let issuer = key("pinned-one");
        let stranger = key("stranger");
        let set = pinned("ignored", &[&issuer]);
        let r = Revocation::issue(&stranger, ACTION, NOW, "not mine to withdraw");
        assert!(revoked(&[r], &set, ACTION, 900, NOW + 1).is_none());
    }

    #[test]
    fn an_edited_withdrawal_is_ignored() {
        let issuer = key("edited-rev");
        let set = pinned("edited", &[&issuer]);
        let mut r = Revocation::issue(&issuer, "another:action", NOW, "");
        r.body.action = ACTION.into(); // widened after signing
        assert!(revoked(&[r], &set, ACTION, 900, NOW + 1).is_none());
    }

    #[test]
    fn everything_issued_before_a_star_withdrawal_is_reached_and_nothing_after() {
        let issuer = key("star");
        let set = pinned("star", &[&issuer]);
        let r = Revocation::issue(&issuer, "*", NOW, "we are standing down for the day");
        assert!(revoked(&[r.clone()], &set, ACTION, 900, NOW + 1).is_some(), "issued before");
        assert!(
            revoked(&[r], &set, ACTION, NOW + 10, NOW + 20).is_none(),
            "an approval given after the withdrawal stands"
        );
    }

    #[test]
    fn a_withdrawal_for_one_action_does_not_reach_another() {
        let issuer = key("one-action");
        let set = pinned("one-action", &[&issuer]);
        let r = Revocation::issue(&issuer, "guard:bbb", NOW, "");
        assert!(revoked(&[r], &set, ACTION, 900, NOW + 1).is_none());
    }
}
