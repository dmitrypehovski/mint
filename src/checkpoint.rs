//! A checkpoint of a log's head, and a second party's signature over it.
//!
//! The receipts in a log are chained, so editing one in place is detectable by anybody holding the
//! log. What no chain can show is that the log presented is *the whole* log: its holder can drop the
//! last ten receipts, present a shorter chain, and every link in it verifies. The runtime's own threat
//! model says so, and so does the standard this format is being shaped after — a self-signed chain
//! cannot answer that question about itself, however long it is.
//!
//! Two small objects close it:
//!
//! - a **checkpoint**: the runtime's own signature over the head of its log — how many receipts there
//!   are, which sequence number the last one carries, and which link hash it ends with;
//! - a **countersignature** by a *different* key over exactly that head, made by a witness that
//!   records what it saw. A later claim of a shorter log, or of a different head at the same sequence
//!   number, is then a contradiction rather than a matter of opinion — and the witness's signature
//!   dates the head without asking the runtime what time it is.
//!
//! What it does not do: it says nothing about the world, only about the log. A witness watching a log
//! that records a lie countersigns the lie. It is evidence about what was recorded, which is exactly
//! as much as a receipt ever claimed.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::receipt::{verify_signature, Receipt, Sealer};

pub const CHECKPOINT_SPEC: &str = "mint.checkpoint/0.1";
pub const WITNESS_SPEC: &str = "mint.witness/0.1";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The head of a log: how much of it there is, and which receipt it ends with.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Head {
    pub count: usize,
    pub head_seq: u64,
    pub head_hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointBody {
    pub spec: String,
    /// The key that holds this log, in the same form a receipt names its agent.
    pub runtime: String,
    pub head: Head,
    pub at_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub body: CheckpointBody,
    pub sig: crate::endorse::Sig,
}

impl Checkpoint {
    /// The bytes that were signed: the body, serialized in declaration order, like a receipt. The
    /// signature is outside them, so a chain of checkpoints commits to content rather than to
    /// signatures over content.
    pub fn canonical(&self) -> String {
        serde_json::to_string(&self.body).unwrap_or_default()
    }

    pub fn verify(&self) -> Result<()> {
        verify_signature(
            &self.body.runtime,
            self.canonical().as_bytes(),
            &self.sig.value,
        )
        .map_err(|e| anyhow!("this checkpoint is not signed by the runtime it names: {e}"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WitnessBody {
    pub spec: String,
    /// The witness's own public key. Holding the receipt key is not witnessing.
    pub witness: String,
    /// Whose log this head belongs to.
    pub runtime: String,
    /// The head as the checkpoint states it, countersigned without editing it.
    pub head: Head,
    /// What this witness saw for this log the time before, when it has seen it before. Kept inside
    /// the signed body so one countersignature is enough to catch a log that shrank.
    pub previously: Option<Head>,
    pub at_ms: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Countersignature {
    pub body: WitnessBody,
    pub sig: crate::endorse::Sig,
    /// The runtime's signature over the checkpoint, copied in so a reader sees both at once.
    pub checkpoint_sig: String,
}

impl Countersignature {
    pub fn canonical(&self) -> String {
        serde_json::to_string(&self.body).unwrap_or_default()
    }
}

/// The head of a published log document.
pub fn head_of(document: &Value) -> Result<Head> {
    let receipts = document
        .get("receipts")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("that document has no receipts in it"))?;
    let last = receipts
        .last()
        .ok_or_else(|| anyhow!("the log is empty — there is no head to checkpoint"))?;
    let r: Receipt = serde_json::from_value(last.clone())?;
    Ok(Head {
        count: receipts.len(),
        head_seq: r.body.seq,
        head_hash: r.link_hash(),
    })
}

/// Sign the head of a log. The runtime does this with its receipt key.
pub fn sign(sealer: &Sealer, head: Head, at_ms: u128) -> Checkpoint {
    let body = CheckpointBody {
        spec: CHECKPOINT_SPEC.to_string(),
        runtime: sealer.agent.clone(),
        head,
        at_ms,
    };
    let canonical = serde_json::to_string(&body).unwrap_or_default();
    let sig = sealer.sign_bytes(canonical.as_bytes());
    Checkpoint {
        body,
        sig: crate::endorse::Sig {
            alg: "ed25519".into(),
            kid: sealer.agent.clone(),
            value: hex(&sig),
        },
    }
}

/// Countersign a checkpoint. Run by the witness, with the witness's own key.
///
/// The checkpoint's own signature is checked first: countersigning an unverified claim would turn the
/// witness's name into a stamp anybody could obtain. `previously` is what this witness saw last time.
pub fn countersign(
    witness: &Sealer,
    cp: &Checkpoint,
    previously: Option<Head>,
    at_ms: u128,
) -> Result<Countersignature> {
    cp.verify()?;
    if witness.agent == cp.body.runtime {
        return Err(anyhow!(
            "the witness holds the same key as the log's runtime: a runtime's second signature over its \
             own head is not a witness, whatever it is called"
        ));
    }
    if let Some(prev) = &previously {
        if let Some(why) = inconsistency(prev, &cp.body.head) {
            return Err(anyhow!("nothing was countersigned — {why}"));
        }
    }
    let body = WitnessBody {
        spec: WITNESS_SPEC.to_string(),
        witness: witness.agent.clone(),
        runtime: cp.body.runtime.clone(),
        head: cp.body.head.clone(),
        previously,
        at_ms,
    };
    let canonical = serde_json::to_string(&body).unwrap_or_default();
    let sig = witness.sign_bytes(canonical.as_bytes());
    Ok(Countersignature {
        body,
        sig: crate::endorse::Sig {
            alg: "ed25519".into(),
            kid: witness.agent.clone(),
            value: hex(&sig),
        },
        checkpoint_sig: cp.sig.value.clone(),
    })
}

/// Why a later head cannot be the continuation of an earlier one.
///
/// A log may grow. It may not shrink, and it may not present a different receipt at a sequence number
/// it has already used: both are the same act — rewriting what was recorded — and a chain alone
/// cannot see either.
pub fn inconsistency(previous: &Head, now: &Head) -> Option<String> {
    if now.count < previous.count {
        return Some(format!(
            "the log held {} receipts and now claims {}: records were taken out",
            previous.count, now.count
        ));
    }
    if now.head_seq < previous.head_seq {
        return Some(format!(
            "the log reached sequence {} and now claims to end at {}",
            previous.head_seq, now.head_seq
        ));
    }
    if now.head_seq == previous.head_seq && now.head_hash != previous.head_hash {
        return Some(format!(
            "sequence {} was checked before with the hash {} and is now presented with {}: the same \
             place in the log holds two different receipts",
            now.head_seq,
            &previous.head_hash[..previous.head_hash.len().min(16)],
            &now.head_hash[..now.head_hash.len().min(16)]
        ));
    }
    None
}

/// What a third party finds when it checks all three objects together.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub ok: bool,
    pub findings: Vec<String>,
}

/// Check a log against its checkpoint and its countersignature.
///
/// `log_head` is where a log presented now ends, or `None` to check the two documents on their own.
/// Everything the check needs is inside the three objects: no service, no network, and no trust in
/// the party asking.
pub fn check(log_head: Option<&Head>, cp: &Checkpoint, cs: &Countersignature) -> Report {
    let mut findings = Vec::new();

    match cp.verify() {
        Ok(()) => {}
        Err(e) => findings.push(format!("the checkpoint does not hold: {e}")),
    }

    match verify_signature(
        &cs.body.witness,
        cs.canonical().as_bytes(),
        &cs.sig.value,
    ) {
        Ok(()) => {}
        Err(e) => findings.push(format!("the countersignature does not hold: {e}")),
    }

    if cs.body.witness == cs.body.runtime {
        findings.push(
            "the witness and the runtime are the same key: this is one party agreeing with itself"
                .to_string(),
        );
    }
    if cs.checkpoint_sig != cp.sig.value {
        findings.push(
            "the countersignature names a different checkpoint signature than the one presented"
                .to_string(),
        );
    }
    if cs.body.runtime != cp.body.runtime {
        findings.push("the countersignature is over another runtime's log".to_string());
    }
    if cs.body.head != cp.body.head {
        findings.push(format!(
            "the countersignature covers a different head ({:?}) than the checkpoint ({:?})",
            cs.body.head, cp.body.head
        ));
    }
    if let Some(prev) = &cs.body.previously {
        if let Some(why) = inconsistency(prev, &cs.body.head) {
            findings.push(why);
        }
    }
    if let Some(seen) = log_head {
        if seen != &cp.body.head {
            findings.push(format!(
                "the log presented ends at {} receipts (sequence {}, hash {}) and the checkpoint says \
                 {} (sequence {}, hash {})",
                seen.count,
                seen.head_seq,
                &seen.head_hash[..seen.head_hash.len().min(16)],
                cp.body.head.count,
                cp.body.head.head_seq,
                &cp.body.head.head_hash[..cp.body.head.head_hash.len().min(16)]
            ));
        }
    }

    Report {
        ok: findings.is_empty(),
        findings,
    }
}

/// A digest of the whole document, for pinning a set of files in one line somewhere else.
pub fn digest_of(text: &str) -> String {
    format!("sha256:{}", hex(&Sha256::digest(text.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealer(name: &str) -> Sealer {
        let path = format!("/tmp/mint-ckpt-test-{name}");
        let _ = std::fs::remove_file(&path);
        Sealer::load_or_create(&path).expect("a key")
    }

    fn head(count: usize, seq: u64, hash: &str) -> Head {
        Head {
            count,
            head_seq: seq,
            head_hash: hash.to_string(),
        }
    }

    #[test]
    fn a_checkpoint_holds_only_for_the_key_that_signed_it() {
        let runtime = sealer("rt1");
        let other = sealer("other1");
        let mut cp = sign(&runtime, head(5, 5, "aa"), 1);
        assert!(cp.verify().is_ok());
        // An edited head is no longer covered by the signature.
        cp.body.head.count = 6;
        assert!(cp.verify().is_err());
        // And a checkpoint signed by somebody else does not speak for this runtime.
        let mut cp2 = sign(&other, head(5, 5, "aa"), 1);
        cp2.body.runtime = runtime.agent.clone();
        assert!(cp2.verify().is_err());
    }

    #[test]
    fn a_witness_must_not_hold_the_runtime_key() {
        let runtime = sealer("rt2");
        let cp = sign(&runtime, head(5, 5, "aa"), 1);
        assert!(countersign(&runtime, &cp, None, 2).is_err());
    }

    #[test]
    fn a_countersignature_covers_the_head_it_names() {
        let runtime = sealer("rt3");
        let witness = sealer("w3");
        let cp = sign(&runtime, head(5, 5, "aa"), 1);
        let cs = countersign(&witness, &cp, None, 2).expect("countersigned");
        assert!(check(Some(&head(5, 5, "aa")), &cp, &cs).ok);

        // The log presented is shorter than the one that was countersigned.
        let short = check(Some(&head(4, 4, "bb")), &cp, &cs);
        assert!(!short.ok);
        assert!(short.findings.iter().any(|f| f.contains("the log presented ends at")));

        // An edited countersigned head no longer holds either.
        let mut cs2 = countersign(&witness, &cp, None, 2).expect("countersigned");
        cs2.body.head = head(6, 6, "cc");
        assert!(!check(None, &cp, &cs2).ok);
    }

    #[test]
    fn a_log_that_shrank_since_the_last_visit_is_caught() {
        let runtime = sealer("rt4");
        let witness = sealer("w4");
        let before = head(10, 10, "aaaaaaaa");
        let cp = sign(&runtime, head(7, 7, "bbbbbbbb"), 2);
        let err = countersign(&witness, &cp, Some(before), 2).unwrap_err().to_string();
        assert!(err.contains("records were taken out"), "{err}");
    }

    #[test]
    fn the_same_sequence_number_with_a_different_hash_is_caught() {
        let runtime = sealer("rt5");
        let witness = sealer("w5");
        let before = head(7, 7, "ffffffff");
        let cp = sign(&runtime, head(7, 7, "eeeeeeee"), 2);
        let err = countersign(&witness, &cp, Some(before), 2).unwrap_err().to_string();
        assert!(err.contains("two different receipts"), "{err}");
    }

    #[test]
    fn growth_is_allowed() {
        let runtime = sealer("rt6");
        let witness = sealer("w6");
        let before = head(7, 7, "aaaaaaaa");
        let cp = sign(&runtime, head(9, 9, "bbbbbbbb"), 2);
        let cs = countersign(&witness, &cp, Some(before), 2).expect("growth is not a fault");
        assert!(check(Some(&head(9, 9, "bbbbbbbb")), &cp, &cs).ok);
    }

    #[test]
    fn a_head_is_read_out_of_a_published_log() {
        // The shape the published document has: a count and a list of signed receipts.
        let doc = serde_json::json!({ "count": 0, "receipts": [] });
        assert!(head_of(&doc).is_err(), "an empty log has no head");
    }
}
