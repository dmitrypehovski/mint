//! The endorsement — the counterparty's signature over the runtime's rules.
//!
//! Each rejection here is a thing a verifier would otherwise report as a counter-signature. The
//! self-endorsement case is the one that matters: a runtime signing its own rules must be rejected
//! for the *reason* (it is not a second party), not because the signature is bad.

mod common;

use common::*;
use levos::endorse;
use levos::policy::Policy;

/// 8. One valid endorsement and four that must be rejected, each for its own reason.
#[test]
fn endorsements_are_accepted_and_rejected_for_the_right_reasons() {
    let rules = Policy::shipped();
    let runtime = sealer("runtime"); // the runtime's own key
    let other = sealer("endorser"); // a second, separate key
    let third = sealer("third");

    // A valid endorsement: a different key, this policy, this runtime, no expiry.
    let good = endorse::sign(&rules, &runtime.agent, "the gate demo", None, "agreed", &other);
    let good_doc = serde_json::to_value(&good).unwrap();
    let accepted = endorse::check(&good_doc, &rules, &runtime.agent, endorse::now_ms())
        .expect("a valid endorsement must be accepted");
    assert_eq!(accepted.body.endorser, other.agent);

    // Signed by the runtime's own key: rejected as self-endorsement, not as a bad signature.
    let self_signed = endorse::sign(&rules, &runtime.agent, "the gate demo", None, "self", &runtime);
    let self_doc = serde_json::to_value(&self_signed).unwrap();
    let err = endorse::check(&self_doc, &rules, &runtime.agent, endorse::now_ms())
        .expect_err("a runtime endorsing its own rules must be rejected");
    assert!(
        err.contains("own key") || err.contains("itself"),
        "the rejection must be about self-endorsement, was: {err}"
    );
    assert!(
        !err.contains("does not verify"),
        "it must NOT be rejected as a signature failure — the signature is valid: {err}"
    );

    // An endorsement of a different policy document.
    let mut other_rules = Policy::shipped();
    other_rules.version = 7;
    let for_other_policy =
        endorse::sign(&other_rules, &runtime.agent, "scope", None, "other rules", &other);
    let op_doc = serde_json::to_value(&for_other_policy).unwrap();
    let err = endorse::check(&op_doc, &rules, &runtime.agent, endorse::now_ms())
        .expect_err("an endorsement of a different policy must be rejected");
    assert!(err.contains("policy"), "was: {err}");

    // An endorsement naming a different runtime.
    let for_other_runtime = endorse::sign(&rules, &third.agent, "scope", None, "other runtime", &other);
    let or_doc = serde_json::to_value(&for_other_runtime).unwrap();
    let err = endorse::check(&or_doc, &rules, &runtime.agent, endorse::now_ms())
        .expect_err("an endorsement naming another runtime must be rejected");
    assert!(err.contains("runtime"), "was: {err}");

    // An expired endorsement.
    let expired = endorse::sign(&rules, &runtime.agent, "scope", Some(1_000), "expired", &other);
    let ex_doc = serde_json::to_value(&expired).unwrap();
    let err = endorse::check(&ex_doc, &rules, &runtime.agent, 2_000)
        .expect_err("an expired endorsement must be rejected");
    assert!(err.contains("expired"), "was: {err}");

    // The endorsement published with the live log is a genuine counter-signature: a different key
    // from the runtime named in the log, over exactly the shipped rules.
    let demo = read_json(&demo_path("gate-endorsement.json"));
    let named_runtime = demo["body"]["runtime"].as_str().unwrap().to_string();
    let at = demo["body"]["at_ms"].as_u64().unwrap() as u128;
    let ok = endorse::check(&demo, &rules, &named_runtime, at)
        .expect("the published endorsement must be a valid counter-signature");
    assert_ne!(
        ok.body.endorser, named_runtime,
        "the endorser must be a different key from the runtime"
    );
    assert_eq!(ok.body.policy_hash, rules.hash());
}
