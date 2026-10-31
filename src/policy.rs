//! The policy, as a document rather than as branches in the code.
//!
//! A receipt carries the hash of the policy it was decided under. That hash is only worth something
//! if the policy behind it is checkable, so the rules live here as data: which observations make a
//! step a read, which make it a write, what happens to a write with no approval, and what happens to
//! a step nobody can classify.
//!
//! The same function decides and re-decides. The gate applies it to reach a verdict; a verifier
//! applies it to the observations recorded in a receipt and checks that the verdict it reaches is the
//! verdict that was recorded. That is the difference between a receipt that says "we decided hold"
//! and one that says "apply these rules yourself to what we recorded, and see whether you get the
//! same answer".
//!
//! What this still does not settle: whether the policy that was published is the policy somebody
//! agreed to. A hash fixes which document was used; it cannot say that the document was wanted. That
//! needs a counter-signature from the other side, which is not built.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SPEC: &str = "mint.policy/0.1";

/// One condition a rule tests, against one named observation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Matcher {
    /// One of: action, tag, role, has_href.
    pub attr: String,
    pub value: String,
}

/// A rule: when every matcher holds, the step is of this class.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rule {
    pub when: Vec<Matcher>,
    pub class: String,
}

/// The document.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    pub spec: String,
    pub version: u32,
    /// What the document means, in the document. A published policy nobody can read is a hash with
    /// no content, and the point of publishing it is that a person can check it says what they want.
    pub note: String,
    /// What a write with no approval bound to it gets: hold or refuse.
    pub on_write_without_approval: String,
    /// What a step that matches no rule gets. `unknown` is a class, not a wildcard.
    pub on_unclassifiable: String,
    /// An approval authorises one performance of one action, not a category of them.
    pub approvals_are_single_use: bool,
    /// Read first, then write. First match wins; no match is unknown, and unknown is not a class
    /// that any rule can assign.
    pub reads: Vec<Rule>,
    pub writes: Vec<Rule>,
}

/// What the runtime observed about an element before deciding. Recorded in every receipt, so a
/// verifier can replay the rules against exactly what the runtime had in front of it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Observed {
    /// click | type
    pub action: String,
    pub tag: String,
    pub role: String,
    /// Does the element point somewhere. A link with somewhere to go changes nothing on the far side.
    pub has_href: bool,
    /// Was an approval presented with this step, and did it match this action.
    pub approval: bool,
}

impl Policy {
    /// The rules that ship. Written to reproduce exactly what the runtime did before the rules were
    /// data, so that publishing the policy changes nothing about behaviour — if it did, the receipts
    /// already issued would stop verifying, which would be its own kind of dishonest.
    pub fn shipped() -> Policy {
        let m = |attr: &str, value: &str| Matcher { attr: attr.into(), value: value.into() };
        Policy {
            spec: SPEC.into(),
            version: 1,
            note: "Reads pass. Anything that commits is held until an approval bound to that exact \
                   action is presented, and released once. A step that matches no rule is refused \
                   even if an approval is offered, because an approval authorises a described \
                   action and nothing here can be described."
                .into(),
            on_write_without_approval: "hold".into(),
            on_unclassifiable: "refuse".into(),
            approvals_are_single_use: true,
            reads: vec![
                // Somewhere to go is a navigation, whatever it looks like.
                Rule { when: vec![m("has_href", "yes")], class: "read".into() },
                // Typing fills a field. The commit, if there is one, is the submit.
                Rule { when: vec![m("action", "type")], class: "read".into() },
                // A link with no destination goes nowhere, so it commits nothing.
                Rule { when: vec![m("tag", "a")], class: "read".into() },
                Rule { when: vec![m("role", "link")], class: "read".into() },
            ],
            writes: vec![
                Rule { when: vec![m("tag", "button")], class: "write".into() },
                Rule { when: vec![m("role", "button")], class: "write".into() },
                // Any other input that survived the read rules commits something.
                Rule { when: vec![m("tag", "input")], class: "write".into() },
            ],
        }
    }

    /// The bytes a hash is taken over. Field order is declaration order, so this is stable.
    pub fn canonical(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn hash(&self) -> String {
        hex(&Sha256::digest(self.canonical().as_bytes()))
    }

    pub fn from_file(path: &str) -> anyhow::Result<Policy> {
        Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
    }
}

impl Observed {
    /// The name used inside a matcher, and the value a matcher compares against.
    ///
    /// `has_href` is written as yes/no rather than true/false so that a matcher is a string in both
    /// halves; a policy document is meant to be readable by whoever is being asked to trust it.
    fn attrs(&self) -> Vec<(&str, String)> {
        vec![
            ("action", self.action.clone()),
            ("tag", self.tag.to_lowercase()),
            ("role", self.role.to_lowercase()),
            ("has_href", if self.has_href { "yes".into() } else { "no".into() }),
        ]
    }

    fn matches(&self, rule: &Rule) -> bool {
        let attrs = self.attrs();
        rule.when.iter().all(|matcher| {
            attrs
                .iter()
                .any(|(name, value)| *name == matcher.attr && *value == matcher.value.to_lowercase())
        })
    }
}

/// The class a step falls into under this policy: read, write, or unknown.
pub fn classify(observed: &Observed, policy: &Policy) -> String {
    for rule in policy.reads.iter().chain(policy.writes.iter()) {
        if observed.matches(rule) {
            return rule.class.clone();
        }
    }
    "unknown".into()
}

/// What the policy says to do, given the class and whether an approval was presented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ruling {
    /// read | write | unknown — what the step would do.
    pub class: String,
    /// allow | hold | refuse — what the policy says about it.
    pub verdict: String,
    /// Prose for a person. Not part of what a verifier checks: the class and the verdict are the
    /// facts, and a sentence cannot be compared without comparing the whole language.
    pub reason: String,
}

pub fn ruling(observed: &Observed, policy: &Policy) -> Ruling {
    let class = classify(observed, policy);
    match class.as_str() {
        "read" => Ruling {
            class,
            verdict: "allow".into(),
            reason: "safe to perform".into(),
        },
        "write" => {
            if observed.approval {
                Ruling {
                    class,
                    verdict: "allow".into(),
                    reason: "an approval bound to this exact action was presented".into(),
                }
            } else {
                Ruling {
                    class,
                    verdict: policy.on_write_without_approval.clone(),
                    reason: "this step commits something and no approval bound to it was presented"
                        .into(),
                }
            }
        }
        _ => Ruling {
            class,
            verdict: policy.on_unclassifiable.clone(),
            reason: "this element is neither a navigation nor a button, so what it would do cannot \
                     be described to anyone; refused rather than tried"
                .into(),
        },
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(action: &str, tag: &str, role: &str, href: bool, approval: bool) -> Observed {
        Observed {
            action: action.into(),
            tag: tag.into(),
            role: role.into(),
            has_href: href,
            approval,
        }
    }

    /// The four kinds the runtime has to tell apart, plus the two answers a write can get.
    #[test]
    fn the_shipped_rules_tell_the_four_kinds_apart() {
        let p = Policy::shipped();
        assert_eq!(ruling(&obs("click", "a", "link", true, false), &p).verdict, "allow");
        assert_eq!(ruling(&obs("type", "input", "textbox", false, false), &p).verdict, "allow");
        assert_eq!(ruling(&obs("click", "button", "button", false, false), &p).verdict, "hold");
        assert_eq!(ruling(&obs("click", "div", "tab", false, false), &p).verdict, "refuse");
        // and a write with an approval goes through
        assert_eq!(ruling(&obs("click", "button", "button", false, true), &p).verdict, "allow");
    }

    /// The policy is data, so changing the document changes the decisions. If this test could not
    /// fail, publishing the policy would be decoration.
    #[test]
    fn the_rules_are_the_thing_that_decides() {
        let mut p = Policy::shipped();
        assert_eq!(ruling(&obs("click", "button", "button", false, false), &p).verdict, "hold");
        p.on_write_without_approval = "refuse".into();
        assert_eq!(ruling(&obs("click", "button", "button", false, false), &p).verdict, "refuse");

        // and a rule that stops calling a button a write changes the class, not just the answer
        let mut q = Policy::shipped();
        q.writes.clear();
        assert_eq!(classify(&obs("click", "button", "button", false, false), &q), "unknown");
    }

    #[test]
    fn the_hash_moves_when_the_rules_do() {
        let a = Policy::shipped();
        let mut b = Policy::shipped();
        assert_eq!(a.hash(), b.hash(), "the same document must hash the same");
        b.version = 2;
        assert_ne!(a.hash(), b.hash(), "a changed document must not hash the same");
    }
}
