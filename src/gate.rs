//! The write gate.
//!
//! A compiled skill runs deterministically, which means it will do the same thing again — including
//! the parts that change the world. Every step therefore crosses this boundary before it touches
//! anything. A step is classified by what it would do; reads pass; writes need an approval bound to
//! that exact action and are released at most once; anything that cannot be classified is refused
//! rather than attempted.
//!
//! Two rules hold the whole thing up.
//!
//! **The decision is recorded before the effect.** A refusal in the log is evidence that nothing
//! happened, and the absence of a decision is evidence that the gate was never asked. A gate that
//! writes its record afterwards proves nothing about what it prevented.
//!
//! **Refusal is the failure direction, never silent execution.** An unrecognised element, a missing
//! approval, a repeated key — each of them ends in a refusal, so a mistake in this file can cause
//! work to be skipped or asked about, but cannot cause an action nobody agreed to.
//!
//! What the approvals are now: signed files, checked before they authorise anything — the issuer must be
//! a key this runtime was told to trust, the signature must hold, the approval must name this exact
//! action, it must be valid now, and it must have been issued for this runtime (`src/approval.rs`). The
//! string form still exists for the rules demonstration, where a visitor's switch stands in for a person,
//! and it is worth nothing wherever approvers are pinned.
//!
//! The decision log becomes signed receipts, and the receipts are hash-chained (`src/receipt.rs`); a
//! checkpoint over the head of that chain, countersigned by a second key, is what a verifier needs in
//! order to be told that the chain it was handed is the whole chain (`src/checkpoint.rs`).
//!
//! What is still not true of it, named rather than left to be discovered: an approval cannot be revoked
//! before its expiry, it cannot be delegated, and the single-use record for a *run* is per-run rather
//! than shared between runs. For the `guard` path an agent uses, single use is read from the log and
//! therefore survives restarts and machines.

use crate::cdp::Element;
use crate::policy;
use crate::receipt;
use crate::skill::{Action, Step};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// What a step would do to the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SideEffect {
    /// Nothing outside the browser changes: navigation, reading, filling a field that is never
    /// submitted.
    Read,
    /// Something commits: a button that acts, a form that is sent, a thing that is deleted or paid.
    Write,
    /// Not recognisable as either. Gated as a write and, by default, refused — because there is
    /// nothing here to describe to a person, so there is nothing they could approve.
    Unknown,
}

impl SideEffect {
    pub fn from_str(s: &str) -> SideEffect {
        match s {
            "read" => SideEffect::Read,
            "write" => SideEffect::Write,
            _ => SideEffect::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SideEffect::Read => "read",
            SideEffect::Write => "write",
            SideEffect::Unknown => "unknown",
        }
    }
}

/// What to do with a step that is not allowed outright.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disposition {
    /// Stop. Nothing is performed and the run says so.
    Refuse,
    /// Do not perform, but report that this one could go ahead with approval.
    Hold,
}

/// The gate's answer for one step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "lowercase")]
pub enum Verdict {
    /// Written as an object with no reason, so every verdict in the log has the same shape. A log
    /// where an allow is a bare string and a refusal is an object cannot be read by anything that
    /// was not written to expect exactly that.
    Allow {},
    Hold { reason: String },
    Refuse { reason: String },
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Allow {} => "allow",
            Verdict::Hold { .. } => "hold",
            Verdict::Refuse { .. } => "refuse",
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Verdict::Allow {} => "",
            Verdict::Hold { reason } | Verdict::Refuse { reason } => reason,
        }
    }

    pub fn performed(&self) -> bool {
        matches!(self, Verdict::Allow {})
    }
}

/// How strict this deployment is. The defaults are the strict ones.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    /// A write with no approval bound to it.
    pub write_without_approval: Disposition,
    /// A step that cannot be classified either way.
    pub unknown: Disposition,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            // Held rather than refused: the caller may be about to present an approval, and
            // saying "this one needs you" is more useful than saying "no".
            write_without_approval: Disposition::Hold,
            // Refused, because an unclassifiable step cannot be described to whoever would
            // approve it. Failing closed here is the whole point of the category.
            unknown: Disposition::Refuse,
        }
    }
}

/// One line of the decision log. Written before the effect; closed afterwards with the outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Decision {
    pub step: usize,
    pub identity: String,
    pub key: String,
    pub side_effect: SideEffect,
    pub verdict: Verdict,
    pub at_ms: u128,
    /// Filled in after the effect, when there was one.
    pub outcome: Option<String>,
    pub element: String,
    /// Everything the policy needs in order to reach the same verdict again.
    pub observed: policy::Observed,
}

/// Classify what a step would do, from the element it resolved to.
///
/// This is deliberately conservative and deliberately explained. A link with somewhere to go is a
/// navigation and changes nothing on the far side. Typing into a field does not commit it; the
/// commit is the submit. A button is where a commit happens. Anything else is unknown, and unknown
/// is refused.
pub fn classify(action: &Action, el: &Element) -> SideEffect {
    let observed = observe(action, el, false);
    SideEffect::from_str(&policy::classify(&observed, &policy::Policy::shipped()))
}

/// What the runtime saw, in the vocabulary the policy's matchers use.
pub fn observe(action: &Action, el: &Element, approval: bool) -> policy::Observed {
    policy::Observed {
        action: match action {
            Action::Click => "click".into(),
            Action::Type { .. } => "type".into(),
        },
        tag: el.tag.clone(),
        role: el.role.clone(),
        has_href: !el.href.trim().is_empty(),
        approval,
    }
}

/// The identity of one logical action: what it is, on which page, in which step of which skill.
///
/// An approval is bound to this string. That is what stops an approval for one action authorising
/// a different one — the substitution attack that a page can attempt between the moment a person
/// agrees and the moment the action runs.
pub fn action_identity(host: &str, step: &Step, index: usize) -> String {
    let action = match &step.action {
        Action::Click => "click".to_string(),
        Action::Type { text } => format!("type:{}", text.len()),
    };
    let canonical = format!(
        "v1|{host}|{}|{}|{action}|{}|{}|{}|{}|{}",
        step.learned_page,
        index,
        step.role,
        step.href_kind,
        step.name_shape,
        step.ordinal,
        step.seen_label
    );
    hex(&Sha256::digest(canonical.as_bytes()))
}

/// The key that makes a replay of the same logical action recognisable as a repeat.
///
/// It must be identical across replays of one effect and different across different ones. That is
/// the entire property: a key that is regenerated per attempt makes duplicate protection vanish,
/// and a key shared between two different actions refuses the second one — a visible refusal,
/// never a silent second execution.
pub fn idempotency_key(host: &str, step: &Step, index: usize) -> String {
    let canonical = format!("idem|v1|{host}|{index}|{}", action_identity(host, step, index));
    hex(&Sha256::digest(canonical.as_bytes()))[..32].to_string()
}

/// The approval token that authorises exactly one action.
pub fn approval_for(identity: &str) -> String {
    format!("approve:{identity}")
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The boundary itself.
pub struct Gate {
    /// The published rules, as they were when this run started.
    policy: policy::Policy,
    released: HashSet<String>,
    decisions: Vec<Decision>,
    host: String,
    /// When signing is on, every decision and every outcome becomes a signed receipt. The gate
    /// works without it; the point of turning it on is that the run can then be checked by someone
    /// who trusts neither the runtime nor its operator.
    receipts: Option<receipt::ReceiptLog>,
    /// Which decision receipt belongs to which idempotency key, so an outcome can point back at
    /// the decision that allowed it.
    decision_receipts: HashMap<String, String>,
    /// The approvers this runtime was told to trust, and the approvals it was handed. Pinned keys
    /// rather than a token anybody can type: whoever says yes has to be somebody the run knows.
    approvers: crate::approval::Approvers,
    presented: Vec<crate::approval::Approval>,
    /// Who authorised the last action that needed it, so the receipt can name them.
    last_approver: Option<String>,
}

impl Gate {
    /// Learn, or run, under a document other than the shipped one.
    ///
    /// The shipped rules hold anything that commits, which is right for replaying a skill: the run is
    /// meant to need nobody's permission. Learning is the other way round — a person is present and
    /// has just asked for this task by name, so the writes the task needs are taken under a document
    /// that says so, and every decision is still written down.
    pub fn adopt_policy(&mut self, rules: policy::Policy) {
        self.policy = rules;
    }

    /// Pin the approvers whose signatures this run will accept.
    pub fn trust_approvers(&mut self, approvers: crate::approval::Approvers) {
        self.approvers = approvers;
    }

    /// Hand the run the approvals it may use. Each is checked against the action waiting before it
    /// authorises anything.
    pub fn present_approvals(&mut self, approvals: Vec<crate::approval::Approval>) {
        self.presented = approvals;
    }

    /// Who authorised the last action that needed an approval.
    pub fn last_approver(&self) -> Option<String> {
        self.last_approver.clone()
    }

    pub fn new(host: &str) -> Self {
        Gate {
            policy: policy::Policy::shipped(),
            released: HashSet::new(),
            decisions: Vec::new(),
            host: host.to_string(),
            receipts: None,
            approvers: crate::approval::Approvers::empty(),
            presented: Vec::new(),
            last_approver: None,
            decision_receipts: HashMap::new(),
        }
    }

    /// A gate that decides by a document other than the one that ships. Used by tests and by
    /// anything that has agreed a policy of its own.
    pub fn with_policy(host: &str, rules: policy::Policy) -> Self {
        let mut g = Gate::new(host);
        g.policy = rules;
        g
    }

    /// The rules this gate decides by. Read it to sign the log with the right policy hash.
    pub fn policy(&self) -> &policy::Policy {
        &self.policy
    }

    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// Start signing. Every decision from here on gets a receipt before it is taken, and every
    /// performed action gets a second one afterwards.
    pub fn sign_with(&mut self, log: receipt::ReceiptLog) {
        self.receipts = Some(log);
    }

    pub fn receipt_log(&self) -> Option<&receipt::ReceiptLog> {
        self.receipts.as_ref()
    }

    pub fn receipt_log_mut(&mut self) -> Option<&mut receipt::ReceiptLog> {
        self.receipts.as_mut()
    }

    fn action_ref(
        &self,
        step: &Step,
        index: usize,
        el: &Element,
        observed: &policy::Observed,
        class: SideEffect,
        key: &str,
    ) -> receipt::ActionRef {
        receipt::ActionRef {
            identity: action_identity(&self.host, step, index),
            key: key.to_string(),
            side_effect: class.as_str().to_string(),
            element: format!("{} \"{}\"", el.tag, el.name),
            params_hash: receipt::params_hash_of(&step.action),
            step: index,
            observed: observed.clone(),
        }
    }

    /// How many steps of each verdict the log holds.
    pub fn tally(&self) -> (usize, usize, usize) {
        let mut allow = 0;
        let mut hold = 0;
        let mut refuse = 0;
        for d in &self.decisions {
            match d.verdict {
                Verdict::Allow {} => allow += 1,
                Verdict::Hold { .. } => hold += 1,
                Verdict::Refuse { .. } => refuse += 1,
            }
        }
        (allow, hold, refuse)
    }

    /// Decide, and write the decision down, before anything is performed.
    ///
    /// `approval` is a token from whoever is entitled to authorise this action. It is checked
    /// against the identity of *this* action, so an approval for something else is worth nothing.
    pub fn decide(
        &mut self,
        step: &Step,
        index: usize,
        el: &Element,
        approval: Option<&str>,
    ) -> (Verdict, String) {
        let identity = action_identity(&self.host, step, index);
        let key = idempotency_key(&self.host, step, index);

        // Was an approval presented, and does it name this exact action. The policy is asked about
        // the answer, not about how it was reached — matching a token is state, not a rule.
        // Who said yes, and whether that answer is worth anything.
        //
        // Where approvers are pinned, only a signed approval from one of them counts. A token anybody
        // could have typed is not accepted even when it is the exact string this gate would have
        // produced: the string was never the permission, the issuer was.
        let mut approved_by: Option<String> = None;
        let authorised = if self.approvers.allows_tokens() {
            // Nothing is pinned, so this is the rules demonstration, where a visitor's switch stands in
            // for a person and the question being asked is what the rules say rather than who said yes.
            approval.map(|a| a == approval_for(&identity)).unwrap_or(false)
        } else {
            let agent = self
                .receipts
                .as_ref()
                .map(|log| log.agent.clone())
                .unwrap_or_default();
            let now = crate::endorse::now_ms();
            for presented in &self.presented {
                match crate::approval::check(presented, &self.approvers, &identity, &agent, now) {
                    crate::approval::Check::Accepted { name, .. } => {
                        approved_by = Some(name);
                        break;
                    }
                    crate::approval::Check::Refused { why } => {
                        // Said out loud rather than passed over: an approval that was presented and
                        // did not hold is the most interesting line in the log.
                        eprintln!("  an approval was presented and does not hold: {why}");
                    }
                }
            }
            approved_by.is_some()
        };
        self.last_approver = approved_by;
        // Which reading of the action applies is the surface's to say. A page's element is read from
        // its role and where it points; a shell command is read from what the command does to the
        // files it names. The rules are the same document either way — that is the whole claim.
        let observed = if el.tag == "command" {
            crate::shell::observed(step.cmd.as_deref().unwrap_or(""), authorised)
        } else {
            observe(&step.action, el, authorised)
        };
        let ruling = policy::ruling(&observed, &self.policy);
        let class = SideEffect::from_str(&ruling.class);

        // Why the verdict is what it is. A rule can look at the page; it cannot look at what this
        // run has already done, so a refusal from the released set is recorded as coming from the
        // run rather than from the policy — otherwise a verifier would expect the rules to
        // reproduce something they cannot see.
        let decided_by = if self.released.contains(&key) { "once-only" } else { "policy" };
        let verdict = if self.released.contains(&key) {
            // The one thing a replayer must never do twice. A rule cannot see this: it depends on
            // what this run has already done, not on what the page looks like.
            Verdict::Refuse {
                reason: "this action was already released once; a repeat is not performed".into(),
            }
        } else {
            match ruling.verdict.as_str() {
                "allow" => Verdict::Allow {},
                other => {
                    // The verdict comes from the published document; the prose may name the
                    // identity, which a document cannot.
                    let reason = match class {
                        SideEffect::Write if !authorised => format!(
                            "this step commits something; it needs an approval for {}",
                            &identity[..12]
                        ),
                        _ => ruling.reason.clone(),
                    };
                    if other == "hold" {
                        Verdict::Hold { reason }
                    } else {
                        Verdict::Refuse { reason }
                    }
                }
            }
        };

        if verdict.performed() {
            self.released.insert(key.clone());
        }
        self.decisions.push(Decision {
            step: index,
            identity,
            key: key.clone(),
            side_effect: class,
            verdict: verdict.clone(),
            at_ms: now_ms(),
            outcome: None,
            element: format!("{} \"{}\"", el.tag, el.name),
            observed: observed.clone(),
        });

        // A receipt for the decision, signed now, before anything could be performed. For a step
        // that is refused or held this is the entire record — and the fact that no outcome receipt
        // follows it is the evidence that nothing happened.
        let aref = self.action_ref(step, index, el, &observed, class, &key);
        let dref = receipt::DecisionRef {
            verdict: verdict.as_str().to_string(),
            reason: verdict.reason().to_string(),
            decided_by: decided_by.to_string(),
        };
        let at = self.decisions.last().map(|d| d.at_ms).unwrap_or(0);
        if let Some(log) = self.receipts.as_mut() {
            let rec = log.append_decision(aref, dref, at);
            self.decision_receipts.insert(key.clone(), rec.body.id);
        }
        (verdict, key)
    }

    /// Close the record of a performed step, and sign a second receipt that points back at the
    /// decision which allowed it. Paired receipts are what let a reader see, without trusting
    /// anyone, that a decision came first and an effect came after.
    pub fn record_outcome(&mut self, key: &str, ok: bool, note: &str) {
        if let Some(d) = self.decisions.iter_mut().rev().find(|d| d.key == key) {
            d.outcome = Some(note.to_string());
        }
        let Some(decision_id) = self.decision_receipts.get(key).cloned() else { return };
        let Some(dec) = self.decisions.iter().rev().find(|d| d.key == key).cloned() else { return };
        if self.receipts.is_none() {
            return;
        }
        let aref = receipt::ActionRef {
            identity: dec.identity.clone(),
            key: key.to_string(),
            side_effect: dec.side_effect.as_str().to_string(),
            element: dec.element.clone(),
            step: dec.step,
            observed: dec.observed.clone(),
            params_hash: None,
        };
        let dref = receipt::DecisionRef {
            verdict: dec.verdict.as_str().to_string(),
            reason: dec.verdict.reason().to_string(),
            decided_by: "policy".to_string(),
        };
        let status = if ok { "completed" } else { "failed" };
        if let Some(log) = self.receipts.as_mut() {
            log.append_outcome(decision_id, aref, dref, status, note);
        }
    }


    /// The steps the gate did not allow, each with the reason it gave.
    ///
    /// A refusal that cannot be read afterwards is indistinguishable from a crash, so this is part
    /// of the report and not an internal detail.
    pub fn blocked(&self) -> Vec<serde_json::Value> {
        self.decisions
            .iter()
            .filter(|d| !matches!(d.verdict, Verdict::Allow {}))
            .map(|d| {
                serde_json::json!({
                    "step": d.step + 1,
                    "side_effect": d.side_effect.as_str(),
                    "verdict": d.verdict.as_str(),
                    "reason": d.verdict.reason(),
                    "element": d.element,
                })
            })
            .collect()
    }

    /// The log as it would be handed to someone who was not there.
    pub fn report(&self) -> serde_json::Value {
        let (allow, hold, refuse) = self.tally();
        serde_json::json!({
            "host": self.host,
            "allowed": allow,
            "held": hold,
            "refused": refuse,
            "decisions": self.decisions,
        })
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
