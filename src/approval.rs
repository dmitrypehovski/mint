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
