//! The receipt properties the project rests on.
//!
//! Every test here is written so that it fails if the property it names is broken. Where a check
//! could pass on an empty or tampered log by accident, the honest version is asserted first (so a
//! broken verifier that always says "ok" is caught by a negative case, and a verifier that always
//! says "broken" is caught by the positive one).

mod common;

use common::*;
use mint::policy::{Observed, Policy};
use mint::receipt::{self, Receipt, ReceiptLog, Sealer};

fn demo_log() -> serde_json::Value {
    read_json(&demo_path("gate-receipts.json"))
}

fn demo_policy() -> Policy {
    Policy::from_file(demo_path("gate-policy.json").to_str().unwrap()).expect("the published rules")
}

fn obs(action: &str, tag: &str, role: &str, href: bool, approval: bool) -> Observed {
    Observed {
        action: action.into(),
        tag: tag.into(),
        role: role.into(),
        has_href: href,
        approval,
    }
}

/// 1. A signed log verifies — and it verifies with the rules it was decided under.
#[test]
fn a_signed_log_verifies_with_the_rules_it_was_decided_under() {
    let doc = demo_log();
    let rules = Policy::shipped();

    let report = receipt::verify(&doc, Some(&rules));
    assert!(
        report.ok,
        "the published log must verify against the shipped rules: {report:?}"
    );
    assert_eq!(report.checked, 7, "each of the 7 receipts must hold");
    assert_eq!(report.replayed, 7, "each must be reproduced from the rules");
    assert!(report.first_break.is_none());
    assert!(report.policy_break.is_none());

    // The log names the policy it was decided under, and that name is the document in hand...
    assert_eq!(
        doc["policy_hash"].as_str().unwrap(),
        rules.hash(),
        "the log must name the policy it was decided under"
    );
    // ...and the document published on disk is that same policy.
    assert_eq!(
        demo_policy().hash(),
        rules.hash(),
        "the published rules document must be the shipped policy, byte for byte"
    );
}

/// 2. Editing a byte of any receipt breaks verification, as a signature failure.
#[test]
fn editing_a_receipt_breaks_verification() {
    let doc = demo_log();
    let rules = Policy::shipped();
    assert!(receipt::verify(&doc, Some(&rules)).ok, "precondition");

    let mut edited = doc.clone();
    // Change the recorded prose of the third receipt and leave its signature exactly as it was.
    edited["receipts"][2]["body"]["decision"]["reason"] =
        serde_json::json!("this line was edited after it was signed");

    let report = receipt::verify(&edited, Some(&rules));
    assert!(!report.ok, "an edited receipt must not verify: {report:?}");
    let brk = report.first_break.expect("a break must be named");
    assert_eq!(brk.seq, 2, "the break must be at the edited receipt");
    assert!(
        brk.reason.contains("signature"),
        "the break must be a signature failure, was: {}",
        brk.reason
    );
    assert_eq!(report.replayed, 0, "no receipt may be replayed after a break");
}

/// 3. Deleting a receipt from the middle breaks the chain.
#[test]
fn deleting_a_middle_receipt_breaks_the_chain() {
    let doc = demo_log();
    let rules = Policy::shipped();
    assert!(receipt::verify(&doc, Some(&rules)).ok, "precondition");

    let mut cut = doc.clone();
    cut["receipts"]
        .as_array_mut()
        .unwrap()
        .remove(3); // drop receipt seq 3; neighbours remain

    let report = receipt::verify(&cut, Some(&rules));
    assert!(!report.ok, "a log with a receipt cut out must not verify: {report:?}");
    let brk = report.first_break.expect("a break must be named");
    assert!(
        brk.reason.contains("chain is broken"),
        "the break must be a chain-link failure, was: {}",
        brk.reason
    );
}

/// 4. Reordering two receipts breaks the chain.
#[test]
fn reordering_two_receipts_breaks_the_chain() {
    let doc = demo_log();
    let rules = Policy::shipped();
    assert!(receipt::verify(&doc, Some(&rules)).ok, "precondition");

    let mut swapped = doc.clone();
    swapped["receipts"].as_array_mut().unwrap().swap(2, 3);

    let report = receipt::verify(&swapped, Some(&rules));
    assert!(!report.ok, "reordered receipts must not verify: {report:?}");
    let brk = report.first_break.expect("a break must be named");
    assert!(
        brk.reason.contains("chain is broken"),
        "the break must be a chain-link failure, was: {}",
        brk.reason
    );
}

/// Build one honest log: a single write, no approval, correctly held.
fn honest_write_log(sealer: Sealer, rules: &Policy) -> ReceiptLog {
    let mut log = ReceiptLog::new("it-test-host", rules, sealer);
    let aref = receipt::ActionRef {
        identity: "id-0001".into(),
        key: "key-0001".into(),
        side_effect: "write".into(),
        element: "button \"Submit the filing\"".into(),
        step: 0,
        observed: obs("click", "button", "button", false, false),
        params_hash: None,
    };
    let dref = receipt::DecisionRef {
        verdict: "hold".into(),
        reason: "this step commits something and no approval bound to it was presented".into(),
        decided_by: "policy".into(),
    };
    log.append_decision(aref, dref, 1_000);
    log
}

/// 5. Rewriting a verdict and re-signing everything with the correct key is caught by re-applying
/// the rules to the recorded observations — not by the signature.
#[test]
fn a_rewritten_verdict_resigned_with_the_key_is_caught_by_replay_not_the_signature() {
    let rules = Policy::shipped();
    let key = temp_key_path("forge");

    let honest = honest_write_log(Sealer::load_or_create(&key).unwrap(), &rules);
    let honest_doc = honest.document();
    let before = receipt::verify(&honest_doc, Some(&rules));
    assert!(before.ok && before.replayed == 1, "precondition: {before:?}");

    // Forge: flip the recorded verdict from the honest "hold" to "allow", then re-sign the whole
    // log with the same key so every signature and every link is valid again. This is exactly what
    // a holder of the key can do, and it defeats every signature-based check.
    let mut forged: Vec<Receipt> = honest_doc["receipts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .collect();
    let mut flipped = 0;
    for r in forged.iter_mut() {
        if r.body.decision.verdict == "hold" {
            r.body.decision.verdict = "allow".into();
            flipped += 1;
        }
    }
    assert_eq!(flipped, 1, "the forge must actually change a verdict");

    let resigned = ReceiptLog::adopt("it-test-host", &rules, Sealer::load_or_create(&key).unwrap(), forged);
    let forged_doc = resigned.document();

    // The signature and the chain are intact: with no rules to replay, the forged log looks fine.
    let no_rules = receipt::verify(&forged_doc, None);
    assert!(
        no_rules.ok,
        "the forged log's signatures must verify — that is the point: the signature alone cannot \
         catch this: {no_rules:?}"
    );

    // But applying the published rules to what the receipt recorded catches it.
    let with_rules = receipt::verify(&forged_doc, Some(&rules));
    assert!(
        !with_rules.ok,
        "re-applying the rules must catch a verdict the rules do not support: {with_rules:?}"
    );
    assert!(
        with_rules.first_break.is_none(),
        "the break must NOT be a signature failure — the signature is valid: {with_rules:?}"
    );
    let brk = with_rules
        .policy_break
        .expect("the replay must name the disagreement");
    assert!(
        brk.reason.contains("'allow'") && brk.reason.contains("'hold'"),
        "the reason must name both verdicts, was: {}",
        brk.reason
    );

    std::fs::remove_file(&key).ok();
}

/// 6. Substituting a different policy document after signing is caught by the hash.
#[test]
fn substituting_a_different_policy_is_caught_by_the_hash() {
    let doc = demo_log();

    let mut other = Policy::shipped();
    other.version = 2; // a genuinely different document, same rules otherwise
    assert_ne!(other.hash(), Policy::shipped().hash(), "precondition");

    let report = receipt::verify(&doc, Some(&other));
    assert!(!report.ok, "the log must not verify against a policy it was not decided under");
    let brk = report.policy_break.expect("a policy break must be named");
    assert!(
        brk.reason.contains("policy") && brk.reason.contains("hashes to"),
        "the break must be a policy-hash mismatch, was: {}",
        brk.reason
    );

    // And the honest policy still verifies, so the failure is the substitution, not the log.
    assert!(
        receipt::verify(&doc, Some(&Policy::shipped())).ok,
        "the untampered log must still verify against its own rules"
    );
}

/// 7 (in miniature, over the real artifact). The shipped rules reproduce the verdicts in the
/// published log exactly. If this fails, the "18/18 replayed" claim on the site is false.
#[test]
fn the_shipped_rules_reproduce_every_recorded_verdict() {
    let doc = demo_log();
    let rules = Policy::shipped();

    assert_eq!(doc["count"].as_u64().unwrap(), 7, "the published log holds 7 receipts");

    let report = receipt::verify(&doc, Some(&rules));
    assert!(
        report.ok,
        "if this fails the published claims on the site are false: {report:?}"
    );
    assert_eq!(report.checked, 7);
    assert_eq!(
        report.replayed, 7,
        "every receipt must be reproduced from the shipped rules"
    );

    // Independently of verify(): derive each verdict again and compare it to the record.
    for r in doc["receipts"].as_array().unwrap() {
        let b = &r["body"];
        let observed: Observed =
            serde_json::from_value(b["action"]["observed"].clone()).unwrap();
        let said = mint::policy::ruling(&observed, &rules);
        assert_eq!(
            said.class,
            b["action"]["side_effect"].as_str().unwrap(),
            "class must match for receipt {}",
            b["id"]
        );
        if b["decision"]["decided_by"].as_str().unwrap() == "policy" {
            assert_eq!(
                said.verdict,
                b["decision"]["verdict"].as_str().unwrap(),
                "verdict must match for receipt {}",
                b["id"]
            );
        }
    }
}
