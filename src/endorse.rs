//! The other side's signature.
//!
//! A receipt says the runtime decided by a published policy. That still leaves the reader asking who
//! chose the policy. Signing it ourselves cannot answer that — a runtime endorsing its own rules is
//! an author approving their own book. So this is a second, separate key: an endorsement is a
//! statement, made by somebody other than the runtime, that these rules were accepted for this scope,
//! for this runtime, for a stated period.
//!
//! What the mechanism can and cannot do, stated plainly because it is the whole point:
//!
//! - it **can** prove that a key other than the runtime's signed exactly this policy hash, for
//!   exactly this runtime, and that nobody has altered that statement since
//! - it **cannot** make the counterparty real. Running the endorsement with two keys we generated
//!   ourselves demonstrates the mechanism and proves nothing about trust. The strength of the claim
//!   is the strength of whoever holds the second key, and no amount of code changes that
//! - it cannot vouch for the clock: `not_after` is compared against the verifier's own time, and the
//!   receipts' times remain the runtime's
//!
//! A verifier that accepted an endorsement signed by the runtime's own key would be worse than one
//! with no endorsement support at all, because it would report a counter-signature where there is
//! none. That check is therefore not optional.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SPEC: &str = "mint.endorsement/0.1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndorsementBody {
    pub spec: String,
    /// The hash of the policy document being accepted.
    pub policy_hash: String,
    /// Whose receipts this covers, by public key. An endorsement of the rules is also an
    /// endorsement of who was using them.
    pub runtime: String,
    /// What the rules are accepted for, in words. A scope of "everything, forever" is a thing a
    /// reader should be able to see for what it is.
    pub scope: String,
    /// Optional. After this moment the endorsement is spent.
    pub not_after_ms: Option<u128>,
    pub endorser: String,
    pub at_ms: u128,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sig {
    pub alg: String,
    pub kid: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endorsement {
    pub body: EndorsementBody,
    pub sig: Sig,
}

impl Endorsement {
    /// The bytes the counterparty signed. Declaration order again, so a verifier reconstructs them.
    pub fn canonical(&self) -> String {
        serde_json::to_string(&self.body).unwrap_or_default()
    }

    pub fn hash(&self) -> String {
        hex(&Sha256::digest(self.canonical().as_bytes()))
    }
}

/// Put a counterparty's signature on a policy.
pub fn sign(
    rules: &crate::policy::Policy,
    runtime: &str,
    scope: &str,
    not_after_ms: Option<u128>,
    note: &str,
    sealer: &crate::receipt::Sealer,
) -> Endorsement {
    let body = EndorsementBody {
        spec: SPEC.into(),
        policy_hash: rules.hash(),
        runtime: runtime.to_string(),
        scope: scope.to_string(),
        not_after_ms,
        endorser: sealer.agent.clone(),
        at_ms: now_ms(),
        note: note.to_string(),
    };
    let canonical = serde_json::to_string(&body).unwrap_or_default();
    let signature = sealer.sign_bytes(canonical.as_bytes());
    Endorsement {
        body,
        sig: Sig { alg: "ed25519".into(), kid: sealer.agent.clone(), value: hex(&signature) },
    }
}

/// Check an endorsement against the log and the rules it is supposed to cover.
///
/// Every rejection here is a thing that would otherwise be reported as a counter-signature. The
/// order matters: identity before content, so that the message a reader gets names the real problem.
pub fn check(
    document: &serde_json::Value,
    rules: &crate::policy::Policy,
    runtime: &str,
    now: u128,
) -> Result<Endorsement, String> {
    let e: Endorsement = serde_json::from_value(document.clone())
        .map_err(|e| format!("the endorsement does not parse: {e}"))?;

    // 1. the endorser signed this, and the signature is by the key it names
    if e.sig.alg != "ed25519" {
        return Err(format!("unknown signature algorithm: {}", e.sig.alg));
    }
    if e.sig.kid != e.body.endorser {
        return Err(
            "the endorsement names one endorser and was signed with another — the two must be the \
             same key"
                .into(),
        );
    }
    crate::receipt::verify_signature(&e.body.endorser, e.canonical().as_bytes(), &e.sig.value)
        .map_err(|why| format!("the endorsement does not verify: {why}"))?;

    // 2. and it is not us. A runtime approving its own rules is not a counter-signature, and
    //    reporting it as one would be worse than having no endorsement support at all.
    if e.body.endorser == runtime {
        return Err(
            "the endorsement was signed by the runtime's own key — that is not a counter-signature, \
             it is the same party agreeing with itself"
                .into(),
        );
    }

    // 3. it is about these rules
    if e.body.policy_hash != rules.hash() {
        return Err(format!(
            "the endorsement covers policy {} but the rules in hand hash to {}",
            &e.body.policy_hash[..12.min(e.body.policy_hash.len())],
            &rules.hash()[..12]
        ));
    }

    // 4. and about this runtime
    if e.body.runtime != runtime {
        return Err(format!(
            "the endorsement names runtime {} but this log was written by {}",
            &e.body.runtime[..12.min(e.body.runtime.len())],
            &runtime[..12.min(runtime.len())]
        ));
    }

    // 5. and it has not run out
    if let Some(deadline) = e.body.not_after_ms {
        if now > deadline {
            return Err(format!(
                "the endorsement expired at {}, which is {} before now — a spent endorsement is not \
                 an endorsement",
                deadline,
                now - deadline
            ));
        }
    }

    Ok(e)
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
