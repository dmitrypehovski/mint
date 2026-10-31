//! Signed receipts.
//!
//! The gate decides; this records what it decided in a form someone who was not there can check.
//! A receipt is one small signed object per step, hash-chained so each one commits to the one
//! before it, and signed with Ed25519 so a holder of the public key can verify it offline, without
//! asking us, the runtime, or any service whether it is real.
//!
//! Two receipts are written for an action that was performed: a **decision** before the effect and
//! an **outcome** after it. An action that was refused or held gets the decision receipt and
//! nothing else — and that absence is the point. It is the difference between "we say we refused"
//! and "here is the record that was signed before anything could have happened, and no record of
//! anything happening".
//!
//! What this deliberately does not claim, and says so in the receipt itself: it proves that the
//! record was issued by the holder of a key and has not been altered since. It does not prove the
//! action was correct, that its inputs were true, that anyone approved it, or that the world
//! changed the way the record describes. A receipt that promises more than it can support is worse
//! than no receipt, because a reader cannot tell where the support ends.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// The exact claim a receipt supports. Carried in every log, so nobody has to infer it.
pub const CLAIM: &str = "A receipt proves that this record was issued by the holder of the stated \
key and has not been altered since. It does not prove the action was correct, that its inputs were \
true, that a person approved it, or that the world changed the way the record describes.";

pub const SPEC: &str = "mint.receipt/0.1";

/// What the action was, in terms the gate already uses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionRef {
    pub identity: String,
    pub key: String,
    pub side_effect: String,
    pub element: String,
    pub step: usize,
    /// What the runtime observed, so a verifier can apply the published rules itself instead of
    /// taking the classification on trust.
    pub observed: crate::policy::Observed,
}

/// What was decided about it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecisionRef {
    pub verdict: String,
    pub reason: String,
    /// "policy" — any holder of the document can reach this verdict again from the observations.
    /// "once-only" — the verdict came from what this run had already done, which no rule can see.
    pub decided_by: String,
}

/// Everything a receipt asserts. Signed as a whole; the signature lives outside this struct so the
/// bytes that were signed can be reconstructed exactly from what is stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReceiptBody {
    pub spec: String,
    pub seq: u64,
    pub id: String,
    /// "decision" — written before the effect. "outcome" — written after one.
    pub kind: String,
    /// The decision receipt an outcome refers to, so a chain can be read as pairs.
    pub refers_to: Option<String>,
    pub decided_at_ms: u128,
    pub signed_at_ms: u128,
    pub agent: String,
    pub host: String,
    pub action: ActionRef,
    pub decision: DecisionRef,
    /// completed | failed | (empty, when there was no effect to report)
    pub status: String,
    pub result_hash: Option<String>,
    pub result_note: Option<String>,
    pub policy_hash: String,
    pub prev_hash: Option<String>,
    pub claim: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Sig {
    pub alg: String,
    pub kid: String,
    pub value: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub body: ReceiptBody,
    pub sig: Sig,
}

impl Receipt {
    /// The exact bytes that were signed. A struct's fields serialize in declaration order, so this
    /// is stable across runs, machines and versions of this file as long as the struct is not
    /// reordered — which is why the struct is the canonical form rather than a hand-built string.
    pub fn canonical(&self) -> String {
        serde_json::to_string(&self.body).unwrap_or_default()
    }

    /// The hash the next receipt in the chain commits to. The signature is excluded, matching the
    /// convention that a chain commits to content, not to signatures over content.
    pub fn link_hash(&self) -> String {
        hex(&Sha256::digest(self.canonical().as_bytes()))
    }
}

/// The signing side.
pub struct Sealer {
    key: ed25519_dalek::SigningKey,
    pub agent: String,
}

impl Sealer {
    /// Load the key from disk, or make one. The seed is hex in a file with owner-only permissions;
    /// losing it means old receipts can no longer be tied to this agent, which is a real cost and
    /// the reason the file is not in the repository.
    pub fn load_or_create(path: &str) -> anyhow::Result<Sealer> {
        if Path::new(path).exists() {
            return Sealer::load(path);
        }
        let seed: [u8; 32] = {
            let mut seed = [0u8; 32];
            // Read exactly the bytes needed. A plain read-to-end on /dev/urandom never returns,
            // because that file has no end — the process simply stops there with no error.
            {
                use std::io::Read;
                let mut f = fs::File::open("/dev/urandom")?;
                f.read_exact(&mut seed)?;
            }
            if let Some(dir) = Path::new(path).parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(path, hex(&seed))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            }
            seed
        };
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let agent = hex(key.verifying_key().as_bytes());
        Ok(Sealer { key, agent })
    }
}

impl Sealer {
    /// Load an existing key. Never creates one: a key file that appears because somebody ran a
    /// review command is not a key anybody agreed to hold.
    pub fn load(path: &str) -> anyhow::Result<Sealer> {
        let text = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("no key at {path}: {e}"))?;
        Sealer::from_hex_seed(text.trim())
    }

    /// The public identity in a key file, without creating one and without touching the key itself.
    pub fn public_of(path: &str) -> anyhow::Result<String> {
        Ok(Sealer::load(path)?.agent)
    }

    fn from_hex_seed(text: &str) -> anyhow::Result<Sealer> {
        let seed: [u8; 32] = decode_hex(text)?
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("the key file must hold 32 bytes of hex"))?;
        let key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let agent = hex(key.verifying_key().as_bytes());
        Ok(Sealer { key, agent })
    }

    /// Sign bytes with this key. Used for documents other than receipts — an endorsement, for
    /// instance, which is signed by the counterparty rather than by the runtime.
    pub fn sign_bytes(&self, msg: &[u8]) -> [u8; 64] {
        use ed25519_dalek::Signer;
        self.key.sign(msg).to_bytes()
    }
}

/// The log, and the machinery to add to it and to check it.
pub struct ReceiptLog {
    pub host: String,
    pub agent: String,
    pub policy_hash: String,
    /// The document the decisions were taken under, kept whole. A verifier needs the rules
    /// themselves, not a hash of them, or it can only confirm that a hash is internally consistent.
    pub rules: crate::policy::Policy,
    /// The counterparty's endorsement, when there is one. Carried in the log for convenience; the
    /// verifier checks its signature regardless of where it found it, so embedding it adds nothing
    /// that has to be trusted.
    pub endorsement: Option<serde_json::Value>,
    sealer: Sealer,
    receipts: Vec<Receipt>,
}

impl ReceiptLog {
    pub fn new(host: &str, rules: &crate::policy::Policy, sealer: Sealer) -> Self {
        // The hash of the document that was applied. It used to be a hash of the host, which was a
        // field called policy_hash that had nothing to do with any policy.
        let policy_hash = rules.hash();
        ReceiptLog {
            host: host.to_string(),
            agent: sealer.agent.clone(),
            policy_hash,
            rules: rules.clone(),
            endorsement: None,
            sealer,
            receipts: Vec::new(),
        }
    }

    /// Take an existing set of receipts and re-sign them, recomputing every link.
    ///
    /// This is what a forger holding the key would do, and it is the only way to test whether the
    /// policy replay does any work: after it, every signature is valid and the chain is intact.
    pub fn adopt(
        host: &str,
        rules: &crate::policy::Policy,
        sealer: Sealer,
        mut receipts: Vec<Receipt>,
    ) -> ReceiptLog {
        let mut log = ReceiptLog::new(host, rules, sealer);
        let mut prev: Option<String> = None;
        let mut rebuilt = Vec::with_capacity(receipts.len());
        for old in receipts.drain(..) {
            let mut body = old.body;
            body.prev_hash = prev;
            let signed = log.sign(body);
            prev = Some(signed.link_hash());
            rebuilt.push(signed);
        }
        log.receipts = rebuilt;
        log
    }

    /// Attach the counterparty's signature to this log.
    pub fn accept_endorsement(&mut self, e: &crate::endorse::Endorsement) {
        self.endorsement = serde_json::to_value(e).ok();
    }

    pub fn receipts(&self) -> &[Receipt] {
        &self.receipts
    }

    pub fn len(&self) -> usize {
        self.receipts.len()
    }

    fn sign(&self, body: ReceiptBody) -> Receipt {
        use ed25519_dalek::Signer;
        let canonical = serde_json::to_string(&body).unwrap_or_default();
        let signature = self.sealer.key.sign(canonical.as_bytes());
        Receipt {
            body,
            sig: Sig {
                alg: "ed25519".to_string(),
                kid: self.sealer.agent.clone(),
                value: hex(&signature.to_bytes()),
            },
        }
    }

    fn next_seq(&self) -> u64 {
        self.receipts.len() as u64
    }

    fn prev_hash(&self) -> Option<String> {
        self.receipts.last().map(|r| r.link_hash())
    }

    /// A receipt for the decision itself, written before anything is performed.
    #[allow(clippy::too_many_arguments)]
    pub fn append_decision(
        &mut self,
        action: ActionRef,
        decision: DecisionRef,
        decided_at_ms: u128,
    ) -> Receipt {
        let seq = self.next_seq();
        let id = format!("{}-{:04}", &self.agent[..12.min(self.agent.len())], seq);
        let body = ReceiptBody {
            spec: SPEC.to_string(),
            seq,
            id: id.clone(),
            kind: "decision".to_string(),
            refers_to: None,
            decided_at_ms,
            signed_at_ms: now_ms(),
            agent: self.agent.clone(),
            host: self.host.clone(),
            action,
            decision,
            status: String::new(),
            result_hash: None,
            result_note: None,
            policy_hash: self.policy_hash.clone(),
            prev_hash: self.prev_hash(),
            claim: CLAIM.to_string(),
        };
        self.dispatch(body)
    }

    /// A receipt for what happened, referring back to the decision that allowed it.
    pub fn append_outcome(
        &mut self,
        refers_to: String,
        action: ActionRef,
        decision: DecisionRef,
        status: &str,
        note: &str,
    ) -> Receipt {
        let seq = self.next_seq();
        let id = format!("{}-{:04}", &self.agent[..12.min(self.agent.len())], seq);
        let body = ReceiptBody {
            spec: SPEC.to_string(),
            seq,
            id,
            kind: "outcome".to_string(),
            refers_to: Some(refers_to),
            decided_at_ms: now_ms(),
            signed_at_ms: now_ms(),
            agent: self.agent.clone(),
            host: self.host.clone(),
            action,
            decision,
            status: status.to_string(),
            result_hash: Some(hex(&Sha256::digest(note.as_bytes()))),
            result_note: Some(note.to_string()),
            policy_hash: self.policy_hash.clone(),
            prev_hash: self.prev_hash(),
            claim: CLAIM.to_string(),
        };
        self.dispatch(body)
    }

    fn dispatch(&mut self, body: ReceiptBody) -> Receipt {
        let receipt = self.sign(body);
        self.receipts.push(receipt.clone());
        receipt
    }

    /// The whole log as one object, ready to be written out and handed over.
    pub fn document(&self) -> serde_json::Value {
        serde_json::json!({
            "spec": SPEC,
            "host": self.host,
            "agent": self.agent,
            "policy_hash": self.policy_hash,
            "policy": self.rules,
            "endorsement": self.endorsement,
            "claim": CLAIM,
            "count": self.receipts.len(),
            "receipts": self.receipts,
        })
    }
}

/// What a check found.
#[derive(Clone, Debug, Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    /// Receipts whose signature and link both hold.
    pub checked: usize,
    /// Where the chain first went wrong, and why. `None` when nothing did.
    pub first_break: Option<BreakPoint>,
    /// Receipts the published rules reproduce exactly — same class, same verdict. Zero when no
    /// policy was supplied, in which case this check did not run.
    pub replayed: usize,
    /// Keys with more than one outcome receipt: an effect that happened twice.
    pub repeated_effects: Vec<String>,
    /// What the policy check found, if it ran and found something.
    pub policy_break: Option<BreakPoint>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BreakPoint {
    pub seq: u64,
    pub id: String,
    pub reason: String,
}

/// How restrictive a verdict is. A runtime may be stricter than the rules it published; it is never
/// allowed to be laxer. Allowing what the policy holds is the failure that matters.
fn restrictiveness(verdict: &str) -> u8 {
    match verdict {
        "allow" => 0,
        "hold" => 1,
        _ => 2,
    }
}

fn break_at(receipts: &[Receipt], index: usize, reason: String) -> BreakPoint {
    BreakPoint {
        seq: receipts[index].body.seq,
        id: receipts[index].body.id.clone(),
        reason,
    }
}

/// Check a log end to end: every signature, every link, and — when the rules are supplied — whether
/// they reproduce every verdict that was recorded.
///
/// The replay is the part that makes signing the policy worth anything. A signature says who wrote a
/// record and that nobody has touched it since. It says nothing about whether the record obeys the
/// rules it claims to have been decided under. Reaching the same verdict from the published rules and
/// the recorded observations is what closes that gap: a runtime that quietly allowed what its own
/// policy refuses is caught here, with a valid signature and an intact chain.
pub fn verify(document: &serde_json::Value, rules: Option<&crate::policy::Policy>) -> VerifyReport {
    let empty = |reason: String| VerifyReport {
        ok: false,
        checked: 0,
        first_break: Some(BreakPoint { seq: u64::MAX, id: "?".into(), reason }),
        replayed: 0,
        repeated_effects: Vec::new(),
        policy_break: None,
    };

    let receipts: Vec<Receipt> = match document.get("receipts").and_then(|r| r.as_array()) {
        Some(arr) => {
            let mut out = Vec::new();
            for v in arr {
                match serde_json::from_value::<Receipt>(v.clone()) {
                    Ok(r) => out.push(r),
                    Err(e) => return empty(format!("the log does not parse as receipts: {e}")),
                }
            }
            out
        }
        None => return empty("no receipts in this document".into()),
    };

    let mut checked = 0usize;
    for (i, r) in receipts.iter().enumerate() {
        // The chain: the first receipt has no predecessor, every later one commits to the hash of
        // the one before it.
        let expected_prev = if i == 0 { None } else { Some(receipts[i - 1].link_hash()) };
        if r.body.prev_hash != expected_prev {
            let reason = match &r.body.prev_hash {
                None => "this receipt claims to be first but is not".to_string(),
                Some(h) => format!(
                    "the chain is broken here: it points at {} but the previous receipt hashes to {}",
                    &h[..12.min(h.len())],
                    &expected_prev.as_deref().unwrap_or("nothing")
                        [..12.min(expected_prev.as_deref().unwrap_or("nothing").len())]
                ),
            };
            return VerifyReport {
                ok: false,
                checked,
                first_break: Some(break_at(&receipts, i, reason)),
                replayed: 0,
                repeated_effects: Vec::new(),
                policy_break: None,
            };
        }

        // The signature, over the exact bytes the body serializes to.
        let fail = |reason: String| VerifyReport {
            ok: false,
            checked,
            first_break: Some(break_at(&receipts, i, reason)),
            replayed: 0,
            repeated_effects: Vec::new(),
            policy_break: None,
        };
        let sig_bytes = match decode_hex(&r.sig.value) {
            Ok(b) => b,
            Err(e) => return fail(format!("the signature is not hex: {e}")),
        };
        let Ok(sig_arr) = <[u8; 64]>::try_from(sig_bytes.as_slice()) else {
            return fail("the signature is not 64 bytes".into());
        };
        let key_bytes = match decode_hex(&r.body.agent) {
            Ok(b) => b,
            Err(e) => return fail(format!("the agent key is not hex: {e}")),
        };
        let Ok(key_arr) = <[u8; 32]>::try_from(key_bytes.as_slice()) else {
            return fail("the agent key is not 32 bytes".into());
        };
        let verifying = match ed25519_dalek::VerifyingKey::from_bytes(&key_arr) {
            Ok(v) => v,
            Err(e) => return fail(format!("the agent key is not a public key: {e}")),
        };
        let signature = ed25519_dalek::Signature::from_bytes(&sig_arr);
        if verifying.verify_strict(r.canonical().as_bytes(), &signature).is_err() {
            return fail(
                "the signature does not match this receipt — it was altered after signing, \
                 or signed with another key"
                    .into(),
            );
        }
        checked += 1;
    }

    // Two outcome receipts for one key is the same effect performed twice, which is the one thing a
    // replayer must never do. It is a property of the whole log, so it is checked across it.
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for r in receipts.iter().filter(|r| r.body.kind == "outcome") {
        *seen.entry(r.body.action.key.as_str()).or_insert(0) += 1;
    }
    let repeated_effects: Vec<String> =
        seen.iter().filter(|(_, n)| **n > 1).map(|(k, _)| k.to_string()).collect();

    // And the replay: apply the published rules to what was recorded, and see whether the same
    // verdict comes back out.
    let mut replayed = 0usize;
    let mut policy_break = None;
    if let Some(rules) = rules {
        let want = rules.hash();
        for (i, r) in receipts.iter().enumerate() {
            if r.body.policy_hash != want {
                policy_break = Some(break_at(
                    &receipts,
                    i,
                    format!(
                        "this receipt was decided under policy {} but the policy supplied hashes to \
                         {} — the rules it claims to have followed are not the rules in hand",
                        &r.body.policy_hash[..12.min(r.body.policy_hash.len())],
                        &want[..12]
                    ),
                ));
                break;
            }
            let ruling = crate::policy::ruling(&r.body.action.observed, rules);
            if ruling.class != r.body.action.side_effect {
                policy_break = Some(break_at(
                    &receipts,
                    i,
                    format!(
                        "the rules call this element a {} but the receipt records it as a {}",
                        ruling.class, r.body.action.side_effect
                    ),
                ));
                break;
            }
            // A verdict reached from the run's own state cannot be reproduced from the rules, so for
            // those the test is direction rather than equality: refusing more than the rules require
            // is safe, and allowing more is not.
            let state_dependent = r.body.decision.decided_by == "once-only";
            let too_lax = restrictiveness(&r.body.decision.verdict) < restrictiveness(&ruling.verdict);
            let wrong = if state_dependent {
                too_lax
            } else {
                ruling.verdict != r.body.decision.verdict
            };
            if wrong {
                policy_break = Some(break_at(
                    &receipts,
                    i,
                    format!(
                        "the rules give '{}' for this step but the receipt records '{}'{} — whatever \
                         decided it, it was not the policy that was published",
                        ruling.verdict,
                        r.body.decision.verdict,
                        if state_dependent {
                            ", and a verdict taken from the run's own state may only be stricter, \
                             never laxer"
                        } else {
                            ""
                        }
                    ),
                ));
                break;
            }
            replayed += 1;
        }
    }

    let ok = policy_break.is_none() && repeated_effects.is_empty();
    VerifyReport {
        ok,
        checked,
        first_break: None,
        replayed,
        repeated_effects,
        policy_break,
    }
}

/// Check one Ed25519 signature over one message.
///
/// Shared, so that receipts and anything else signed with the same key type go through a single
/// implementation. Two copies of this is how one of them quietly ends up more permissive.
pub fn verify_signature(agent_hex: &str, msg: &[u8], sig_hex: &str) -> anyhow::Result<()> {
    let sig_bytes = decode_hex(sig_hex)?;
    let sig_arr: [u8; 64] = sig_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("the signature is not 64 bytes"))?;
    let key_bytes = decode_hex(agent_hex)?;
    let key_arr: [u8; 32] = key_bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("the key is not 32 bytes"))?;
    let verifying = ed25519_dalek::VerifyingKey::from_bytes(&key_arr)
        .map_err(|e| anyhow::anyhow!("not a public key: {e}"))?;
    verifying
        .verify_strict(msg, &ed25519_dalek::Signature::from_bytes(&sig_arr))
        .map_err(|_| anyhow::anyhow!("the signature does not match the bytes it covers"))?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn decode_hex(s: &str) -> anyhow::Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        anyhow::bail!("odd number of hex digits");
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = (bytes[i] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("not a hex digit: {}", bytes[i] as char))?;
        let lo = (bytes[i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| anyhow::anyhow!("not a hex digit: {}", bytes[i + 1] as char))?;
        out.push((hi * 16 + lo) as u8);
    }
    Ok(out)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
