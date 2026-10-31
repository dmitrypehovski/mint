//! The write gate and the rules it decides by.

mod common;

use mint::cdp::Element;
use mint::gate::{self, Gate};
use mint::policy::{self, Observed, Policy};
use mint::skill::{Action, Step};

fn observed(action: &str, tag: &str, role: &str, href: bool, approval: bool) -> Observed {
    Observed {
        action: action.into(),
        tag: tag.into(),
        role: role.into(),
        has_href: href,
        approval,
    }
}

fn step_click(role: &str) -> Step {
    Step {
        action: Action::Click,
        role: role.into(),
        href_kind: String::new(),
        name_shape: "button".into(),
        ordinal: 0,
        seen_label: "Submit the filing".into(),
        learned_page: "page-a".into(),
        produced_page: "page-b".into(),
        intent: "submit the filing".into(),
        confidence: 1.0,
    }
}

fn element(tag: &str, role: &str, href: &str) -> Element {
    Element {
        on: true,
        handle: "h1".into(),
        tag: tag.into(),
        role: role.into(),
        name: "Submit the filing".into(),
        href: href.into(),
    }
}

/// 7. require_approval_then_release, at the level of the published rules themselves.
#[test]
fn the_rules_hold_a_write_then_release_it_once() {
    let p = Policy::shipped();

    // a write with no approval is held
    let write = observed("click", "button", "button", false, false);
    assert_eq!(policy::classify(&write, &p), "write");
    assert_eq!(policy::ruling(&write, &p).verdict, "hold");

    // the same write with an approval bound to it is allowed
    let approved = observed("click", "button", "button", false, true);
    assert_eq!(policy::classify(&approved, &p), "write");
    assert_eq!(policy::ruling(&approved, &p).verdict, "allow");

    // an unclassifiable element is refused, and stays refused even with an approval
    let unknown = observed("click", "div", "tab", false, true);
    assert_eq!(policy::classify(&unknown, &p), "unknown");
    assert_eq!(policy::ruling(&unknown, &p).verdict, "refuse");
}

/// 7. The same property end to end through the gate: hold, a wrong approval still holds, the right
/// approval releases exactly once, and an unclassifiable element is refused with approval offered.
#[test]
fn the_gate_holds_then_releases_a_write_exactly_once() {
    let host = "127.0.0.1:0";
    let mut g = Gate::new(host);
    let s = step_click("button");
    let el = element("button", "button", "");

    // no approval -> hold, and nothing is released
    let (v, _) = g.decide(&s, 0, &el, None);
    assert_eq!(v.as_str(), "hold");
    assert!(!g.receipt_log().is_some(), "no signing was configured");

    // an approval for a different action is worth nothing -> still hold
    let identity = gate::action_identity(host, &s, 0);
    let (wrong, _) = g.decide(&s, 1, &el, Some("approve:not-this-action"));
    assert_eq!(wrong.as_str(), "hold", "a foreign approval must not authorise this write");

    // the approval bound to this exact action -> allow
    let token = gate::approval_for(&identity);
    let (allowed, _) = g.decide(&s, 0, &el, Some(&token));
    assert_eq!(allowed.as_str(), "allow");

    // a second attempt at the same action -> refused: released at most once
    let (again, _) = g.decide(&s, 0, &el, Some(&token));
    assert_eq!(again.as_str(), "refuse", "a released write must not be released twice");

    // an unclassifiable element is refused even with a matching approval offered
    let s2 = step_click("tab");
    let el2 = element("div", "tab", "");
    let id2 = gate::action_identity(host, &s2, 0);
    let tok2 = gate::approval_for(&id2);
    let (refused, _) = g.decide(&s2, 0, &el2, Some(&tok2));
    assert_eq!(refused.as_str(), "refuse", "unknown must be refused even with approval");

    let (allow, hold, refuse) = g.tally();
    assert_eq!((allow, hold, refuse), (1, 2, 2), "the tally must match the decisions taken");
}

/// The idempotency key must be stable across replays of one action and different across actions;
/// otherwise duplicate protection either vanishes or refuses an unrelated step.
#[test]
fn idempotency_keys_are_stable_and_action_specific() {
    let host = "h";
    let a = step_click("button");
    let b = step_click("link");

    assert_eq!(
        gate::idempotency_key(host, &a, 0),
        gate::idempotency_key(host, &a, 0),
        "the same action must produce the same key"
    );
    assert_ne!(
        gate::idempotency_key(host, &a, 0),
        gate::idempotency_key(host, &b, 0),
        "different actions must produce different keys"
    );
    assert_ne!(
        gate::idempotency_key(host, &a, 0),
        gate::idempotency_key(host, &a, 1),
        "the same action at a different index is a different logical action"
    );
}
