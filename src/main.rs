//! mint — an agent runtime that does not pay twice for the same decision, and does not
//! pretend a step worked when it did not.
//!
//! First run: a model is asked what to do at every step, and the run is slow. It leaves
//! behind a compiled skill — a small program of typed steps that describe the *shape* of
//! what to act on, not the words that happened to be on screen. Later runs execute that
//! program against live state, on pages whose contents have changed, without calling a
//! model. When a step cannot be recognised, or runs without producing what it promised,
//! the runtime asks for that one step, checks itself again, and writes the correction back
//! into the skill: one mistake costs one question, never a broken run.

use mint::{decide, endorse, gate, policy, receipt, skill, state};

use anyhow::{anyhow, Result};
use mint::cdp::{Cdp, Element};
use mint::skill::{Action, Found, Skill, Step};
use serde_json::json;
use std::time::Instant;

const PORT: u16 = 8830;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn is_text_field(role: &str) -> bool {
    matches!(role, "textbox" | "searchbox" | "combobox")
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str).unwrap_or("help") {
        "probe" => probe(flag(&args, "--url").unwrap_or_else(|| "about:blank".into())).await,
        "learn" => learn(&args).await,
        "race" => race(&args).await,
        "demo" => demo(&args).await,
        "chain" => chain(&args).await,
        "eval" => evaluate(&args).await,
        "discover" => discover(&args).await,
        "traces" => traces(&args).await,
        "arli-payload" => arli_payload(&args).await,
        "gate-demo" => gate_demo(&args).await,
        "live" => live(&args).await,
        "receipts" => receipts_cmd(&args),
        "endorse" => endorse_cmd(&args),
        "rules" => rules_cmd(&args),
        _ => {
            println!("{}", include_str!("help.txt"));
            Ok(())
        }
    }
}

async fn probe(url: String) -> Result<()> {
    let mut cdp = Cdp::connect(PORT).await?;
    let nav_us = cdp.navigate(&url).await?;
    let t = Instant::now();
    let (els, here) = cdp.state().await?;
    let state_us = t.elapsed().as_micros() as u64;
    let payload = serde_json::to_string(&els)?.len();
    let (naive_bytes, naive_nodes) = cdp.naive_payload().await?;
    println!("page: {here}");
    println!("addressable elements: {}", els.len());
    println!("navigation: {:.0} ms | state capture: {:.1} ms", nav_us as f64 / 1000.0, state_us as f64 / 1000.0);
    println!("our state payload: {payload} bytes");
    println!("naive path (page text or the whole accessibility tree): {naive_bytes} bytes, {naive_nodes} nodes");
    println!("difference: {:.0}x smaller", naive_bytes as f64 / payload.max(1) as f64);
    println!("\nfirst elements:\n{}", state::as_table(&els[..els.len().min(12)]));
    Ok(())
}

/// Act on one element, whichever kind of step it is. Returns false when the element has
/// gone from the page, which is a fact the caller has to handle, not an error to raise.
/// The only way an action is ever performed.
///
/// The gate is a required argument, so an action cannot be taken without a decision being written
/// first — that is a property the compiler enforces rather than a promise in a comment. Returns
/// whether it happened and, when it did not, why: a page that refused and a gate that refused need
/// different handling, because one may be worth re-observing and the other must stop the run.
async fn perform(
    cdp: &mut Cdp,
    gate: &mut gate::Gate,
    step: &Step,
    index: usize,
    el: &Element,
    approval: Option<&str>,
) -> Result<(bool, Option<String>)> {
    let (verdict, key) = gate.decide(step, index, el, approval);
    if !verdict.performed() {
        return Ok((false, Some(format!("{}: {}", verdict.as_str(), verdict.reason()))));
    }
    let ok = match &step.action {
        Action::Click => cdp.click_handle(&el.handle).await?,
        Action::Type { text } => cdp.type_handle(&el.handle, text).await?,
    };
    gate.record_outcome(&key, ok, if ok { "performed" } else { "the page did not take it" });
    Ok((ok, None))
}

/// A signing log for a run, or nothing with a note when the key cannot be read.
///
/// A run must never fail because signing is unavailable — but it must never claim to be signed
/// when it is not, which is why the absence is reported rather than silently skipped.
/// Where the signing key lives. One place, so that nothing can quietly sign with a second key.
/// Where the signing keys live.
///
/// Overridable by environment, because a checkout on somebody else's machine should not go looking
/// for a path that only exists on ours. The defaults are where this machine keeps them: outside the
/// repository, so that no clone ever contains a key.
fn receipt_key() -> String {
    std::env::var("MINT_RECEIPT_KEY")
        .unwrap_or_else(|_| "/root/.secrets/mint-receipt.key".into())
}

/// The counterparty's key. Separate from the runtime's on purpose: an endorsement the runtime can
/// produce by itself is not a counter-signature, and this variable exists so that it cannot be one.
fn endorser_key() -> String {
    std::env::var("MINT_ENDORSER_KEY")
        .unwrap_or_else(|_| "/root/.secrets/mint-endorser.key".into())
}

fn sign_for(gate: &mut gate::Gate, host: &str) {
    let rules = gate.policy().clone();
    match receipt::Sealer::load_or_create(&receipt_key()) {
        Ok(sealer) => gate.sign_with(receipt::ReceiptLog::new(host, &rules, sealer)),
        // A run must never fail because signing is unavailable — but it must never claim to be
        // signed when it is not, so the absence is said out loud.
        Err(e) => eprintln!("  receipts are off for this run: {e}"),
    }
}

/// Write this run's receipts, if a directory was given, and say whether they verify.
fn receipt_summary(gate: &gate::Gate, dir: Option<&str>, index: usize) -> serde_json::Value {
    let Some(log) = gate.receipt_log() else {
        return json!({"signed": false, "reason": "signing was unavailable for this run"});
    };
    let doc = log.document();
    let v = receipt::verify(&doc, Some(&log.rules));
    let mut wrote = None;
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir).ok();
        let file = format!("{}/run-{:02}.json", dir.trim_end_matches('/'), index + 1);
        if std::fs::write(&file, serde_json::to_string_pretty(&doc).unwrap_or_default()).is_ok() {
            wrote = Some(file);
        }
    }
    json!({
        "signed": true,
        "count": log.len(),
        "agent": log.agent,
        "verified": v.ok,
        "verified_count": v.checked,
        "break": v.first_break,
        "file": wrote,
    })
}

fn report_verify(r: &receipt::VerifyReport, path: &str) {
    if let Some(b) = &r.policy_break {
        println!("  {path}\n    BROKEN at receipt {} ({}) — the rules disagree", b.seq, b.id);
        println!("    {}", b.reason);
        return;
    }
    if !r.repeated_effects.is_empty() {
        println!("  {path}\n    BROKEN — an effect was performed more than once");
        for k in &r.repeated_effects {
            println!("    the same action, released twice: {k}");
        }
        return;
    }
    match (&r.ok, &r.first_break) {
        (true, _) => {
            let replay = if r.replayed > 0 {
                format!(", and {} of them reproduced from the published rules", r.replayed)
            } else {
                ", though the rules were not replayed".to_string()
            };
            println!(
                "  {path}\n    OK — {} receipts, every signature and every link in the chain holds{replay}",
                r.checked
            );
        }
        (false, Some(b)) => {
            println!("  {path}\n    BROKEN at receipt {} ({})", b.seq, b.id);
            println!("    {}", b.reason);
            println!("    verified before the break: {}", r.checked);
        }
        (false, None) => println!("  {path}\n    BROKEN, and the verifier could not say where"),
    }
}

/// Apply the rules to a set of observations and print what they say.
///
/// The whole policy engine with nothing attached: the same code path the runtime uses to decide, so
/// a third party can re-derive our verdicts without running an agent, a browser or a model. It is
/// also what the demonstration site calls when a visitor flips a switch.
fn rules_cmd(args: &[String]) -> Result<()> {
    let policy_path = flag(args, "--policy").ok_or_else(|| anyhow!("--policy is required"))?;
    let rules = policy::Policy::from_file(&policy_path)?;
    let obs_path =
        flag(args, "--observations").ok_or_else(|| anyhow!("--observations is required"))?;
    let text = if obs_path == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        buf
    } else {
        std::fs::read_to_string(&obs_path)?
    };
    let doc: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| anyhow!("{obs_path} is not JSON: {e}"))?;
    let list = match &doc {
        serde_json::Value::Array(a) => a.clone(),
        v => v
            .get("observations")
            .and_then(|o| o.as_array())
            .cloned()
            .ok_or_else(|| anyhow!("expected an array, or an object with an \"observations\" array"))?,
    };

    let as_json = args.iter().any(|a| a == "--json");
    let mut results = Vec::new();
    for (i, item) in list.iter().enumerate() {
        let observed: policy::Observed = serde_json::from_value(item.clone())
            .map_err(|e| anyhow!("observation {i} is not in the recorded shape: {e}"))?;
        let said = policy::ruling(&observed, &rules);
        if as_json {
            results.push(json!({
                "index": i,
                "observed": observed,
                "class": said.class,
                "verdict": said.verdict,
                "reason": said.reason,
            }));
        } else {
            println!("  {i}. {:<7} {:<6} — {}", said.class, said.verdict, said.reason);
        }
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "spec": policy::SPEC,
                "policy_version": rules.version,
                "policy_hash": rules.hash(),
                "results": results,
            }))?
        );
    } else {
        println!(
            "\n  decided by {} v{}, hash {}",
            policy::SPEC,
            rules.version,
            &rules.hash()[..16]
        );
    }
    Ok(())
}

/// Put a counterparty's signature on a policy: "these rules, for this runtime, in this scope".
///
/// A separate command with a separate key, deliberately. The runtime writing its own endorsement
/// would produce a document that looks like a counter-signature and is not one, which is worse than
/// having none — a reader would stop looking for the missing half.
fn endorse_cmd(args: &[String]) -> Result<()> {
    let policy_path = flag(args, "--policy").ok_or_else(|| anyhow!("--policy is required"))?;
    let rules = policy::Policy::from_file(&policy_path)?;
    let scope = flag(args, "--scope").unwrap_or_else(|| "unspecified".into());
    let note = flag(args, "--note").unwrap_or_default();
    let days: Option<u128> = flag(args, "--days").and_then(|v| v.parse().ok());
    let not_after_ms = days.map(|d| endorse::now_ms() + d * 86_400_000);

    // Whose receipts this covers. Named on the command line, or read from the runtime's key file —
    // a convenience for running both sides on one machine, and the reason the demonstration below
    // says out loud that its trust claim is weak.
    let runtime = match flag(args, "--runtime") {
        Some(r) => r,
        None => {
            let agent = receipt::Sealer::public_of(&receipt_key())?;
            println!("  runtime taken from {}: {}", receipt_key(), &agent[..12.min(agent.len())]);
            agent
        }
    };

    let endorser = receipt::Sealer::load_or_create(&endorser_key())?;
    if endorser.agent == runtime {
        anyhow::bail!(
            "the endorser key and the runtime key are the same key — that is the runtime endorsing              itself, not a counter-signature. Point the endorser at a different key."
        );
    }

    let e = endorse::sign(&rules, &runtime, &scope, not_after_ms, &note, &endorser);
    let out = flag(args, "--out").unwrap_or_else(|| "endorsement.json".into());
    std::fs::write(&out, serde_json::to_string_pretty(&serde_json::to_value(&e)?)?)?;

    println!(
        "  policy {} endorsed by {}, for runtime {}",
        &rules.hash()[..12],
        &e.sig.kid[..12.min(e.sig.kid.len())],
        &runtime[..12.min(runtime.len())]
    );
    println!("  scope:  {scope}");
    match not_after_ms {
        Some(deadline) => println!("  until:  {deadline}"),
        None => println!("  until:  no expiry"),
    }
    println!("\n  what this proves:  that this key signed exactly this policy hash, for this runtime,");
    println!("                     and that nobody has altered that statement since");
    println!("  what it does not:  that the holder of this key is anyone the reader knows.");
    println!("                     The strength of the endorsement is the strength of that key.");
    println!("\n  written to {out}");
    Ok(())
}

/// Check a signed log — and, when asked, check that copies of it that lie all fail.
///
/// The second half matters more than the first. A verifier that has only ever been run on honest
/// input has not been shown to verify anything at all.
fn receipts_cmd(args: &[String]) -> Result<()> {
    let path = flag(args, "--in").ok_or_else(|| anyhow!("--in is required"))?;
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let count = doc.get("receipts").and_then(|r| r.as_array()).map(|a| a.len()).unwrap_or(0);
    let agent = doc.get("agent").and_then(|a| a.as_str()).unwrap_or("?");
    println!("  log of {count} receipts, agent {}", &agent[..12.min(agent.len())]);

    // The rules to check against. From a file when one is given, because that is the independent
    // check; otherwise from the log itself, which only shows that the log agrees with itself. Which
    // one was used is printed, because the difference between them is the whole point.
    let (rules, source) = match flag(args, "--policy") {
        Some(p) => (Some(policy::Policy::from_file(&p)?), format!("from {p}")),
        None => (
            doc.get("policy")
                .and_then(|v| serde_json::from_value::<policy::Policy>(v.clone()).ok()),
            "from the log itself — self-consistency only, not an independent check".to_string(),
        ),
    };
    match &rules {
        Some(r) => println!(
            "  policy {} v{} — {} ({source})",
            &r.hash()[..12],
            r.version,
            if r.note.is_empty() { "no note".into() } else { r.note.chars().take(60).collect::<String>() + "…" }
        ),
        None => println!("  policy: none supplied or found, so the rules were not replayed"),
    }
    if let Some(c) = doc.get("claim").and_then(|c| c.as_str()) {
        println!("  claim: {c}");
    }
    println!();

    let real = receipt::verify(&doc, rules.as_ref());
    report_verify(&real, &path);

    // Whether the rules were accepted by anybody other than the party that wrote them. This is the
    // question a receipt cannot answer on its own, and leaving it unasked is how an honest receipt
    // ends up read as an endorsement of itself.
    let (endorsement, e_source) = match flag(args, "--endorsement") {
        Some(p) => (
            serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&p)?).ok(),
            format!("from {p}"),
        ),
        None => (
            doc.get("endorsement").cloned().filter(|v| !v.is_null()),
            "from the log itself".to_string(),
        ),
    };
    let mut verdict = if real.ok { "log verified" } else { "log BROKEN" };
    if real.replayed > 0 {
        verdict = if real.ok { "log verified, rules replayed" } else { "log BROKEN" };
    }
    match (&endorsement, &rules) {
        (Some(e), Some(rules_in_hand)) => {
            let runtime = doc.get("agent").and_then(|a| a.as_str()).unwrap_or_default();
            match endorse::check(e, rules_in_hand, runtime, endorse::now_ms()) {
                Ok(e) => {
                    println!(
                        "\n  endorsed by {} ({e_source}, a different key from the runtime's)",
                        &e.body.endorser[..12.min(e.body.endorser.len())]
                    );
                    println!("    scope: {}", e.body.scope);
                    match e.body.not_after_ms {
                        Some(d) => println!("    until: {d}"),
                        None => println!("    until: no expiry"),
                    }
                    if !e.body.note.is_empty() {
                        println!("    note:  {}", e.body.note);
                    }
                    if real.ok {
                        println!(
                            "  verdict: {verdict}; the rules were accepted by somebody other than us"
                        );
                    } else {
                        println!(
                            "  verdict: {verdict} — the record itself does not hold, and a \
                             counter-signature on a broken record says nothing either way"
                        );
                    }
                }
                Err(why) => {
                    println!("\n  ENDORSEMENT REJECTED: {why}");
                    if real.ok {
                        println!(
                            "  verdict: {verdict}, but the counter-signature does not hold — so nobody \
                             outside this runtime has been shown to accept these rules"
                        );
                    } else {
                        println!("  verdict: {verdict}");
                    }
                }
            }
        }
        (None, _) => {
            println!(
                "\n  endorsement: none — the rules were published by the same party that applied \
                 them, and nobody else is shown to have accepted them"
            );
            println!("  verdict: {verdict}; not endorsed");
        }
        (Some(_), None) => {
            println!("\n  an endorsement is present but there are no rules in hand to check it against");
            println!("  verdict: {verdict}; endorsement unchecked");
        }
    }

    if let Some(dir) = flag(args, "--tamper") {
        // Five ways a log can lie. A verifier shown only the easiest one has not been shown to
        // verify anything, and "tamper-evident" is a claim that collapses on the second kind.
        let dir = dir.trim_end_matches('/').to_string();
        std::fs::create_dir_all(&dir).ok();
        let mut cases: Vec<(&str, &str, serde_json::Value, Option<policy::Policy>)> = Vec::new();

        // 1. a line edited, the signature left alone
        let mut edited = doc.clone();
        if let Some(arr) = edited.get_mut("receipts").and_then(|r| r.as_array_mut()) {
            for rec in arr.iter_mut() {
                let is_decision = rec.get("body").and_then(|b| b.get("kind")).and_then(|k| k.as_str())
                    == Some("decision");
                if is_decision {
                    rec["body"]["decision"]["reason"] =
                        json!("this line was edited after it was signed");
                    break;
                }
            }
        }
        cases.push(("edited", "a line edited, signature left alone", edited, rules.clone()));

        // 2 and 3. structure removed or reordered
        let mut dropped = doc.clone();
        if let Some(arr) = dropped.get_mut("receipts").and_then(|r| r.as_array_mut()) {
            if arr.len() > 2 {
                arr.remove(1);
            }
        }
        cases.push(("dropped", "a receipt deleted from the middle", dropped, rules.clone()));

        let mut swapped = doc.clone();
        if let Some(arr) = swapped.get_mut("receipts").and_then(|r| r.as_array_mut()) {
            if arr.len() > 2 {
                arr.swap(1, 2);
            }
        }
        cases.push(("swapped", "two receipts reordered", swapped, rules.clone()));

        // 4. the one that matters: a verdict rewritten by someone who holds the key, with the whole
        // chain re-signed. Every signature is valid and every link holds. Only the rules catch it.
        if let Some(used) = rules.clone() {
            let bodies: Vec<receipt::Receipt> = doc
                .get("receipts")
                .and_then(|r| r.as_array())
                .map(|a| a.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect())
                .unwrap_or_default();
            if let Ok(sealer) = receipt::Sealer::load_or_create(&receipt_key()) {
                let mut forged_bodies = bodies;
                let mut changed = false;
                for r in forged_bodies.iter_mut() {
                    if r.body.kind == "decision" && r.body.decision.verdict == "hold" {
                        r.body.decision.verdict = "allow".into();
                        r.body.decision.reason = "allowed".into();
                        changed = true;
                        break;
                    }
                }
                if changed {
                    let host = doc.get("host").and_then(|h| h.as_str()).unwrap_or_default();
                    let log = receipt::ReceiptLog::adopt(host, &used, sealer, forged_bodies);
                    cases.push((
                        "forged",
                        "a held write rewritten to 'allow', then re-signed by the key holder",
                        log.document(),
                        rules.clone(),
                    ));
                }
            }
        }

        // 5. the rules themselves swapped out under a log that was signed against the old ones
        if let Some(used) = rules.clone() {
            let mut other = used.clone();
            other.on_write_without_approval = "refuse".into();
            cases.push((
                "policy",
                "the rules changed after the log was signed",
                doc.clone(),
                Some(other),
            ));
        }

        println!();
        for (slug, what, bad, bad_rules) in cases {
            let copy = format!("{dir}/tampered-{slug}.json");
            std::fs::write(&copy, serde_json::to_string_pretty(&bad)?)?;
            let v = receipt::verify(&bad, bad_rules.as_ref());
            let caught = if let Some(b) = &v.policy_break {
                format!("caught by the rules at receipt {}", b.seq)
            } else if !v.repeated_effects.is_empty() {
                "caught: an effect ran twice".to_string()
            } else if v.ok {
                "NOT CAUGHT — the verifier accepted it".to_string()
            } else {
                format!(
                    "caught at receipt {}",
                    v.first_break.as_ref().map(|b| b.seq.to_string()).unwrap_or_else(|| "?".into())
                )
            };
            println!("  {what}\n      → {caught}");
            let why = v
                .policy_break
                .as_ref()
                .map(|b| b.reason.clone())
                .or_else(|| v.first_break.as_ref().map(|b| b.reason.clone()))
                .unwrap_or_else(|| "an effect was performed more than once".into());
            println!("      {}", &why[..why.len().min(150)]);
        }
        // And the endorsement, which is a separate document with its own ways of lying. Each case
        // is properly signed, so that the check under test is the one that has to fire.
        if let (Some(used), Some(agent)) = (
            rules.clone(),
            doc.get("agent").and_then(|a| a.as_str()).map(|s| s.to_string()),
        ) {
            match (receipt::Sealer::load(&endorser_key()), receipt::Sealer::load(&receipt_key())) {
                (Ok(endorser), Ok(runtime_key)) => {
                    let scope = "the gate demonstration published at mint.arliwork.com";
                    let mut other_policy = used.clone();
                    other_policy.version += 1;
                    let cases: Vec<(&str, &str, endorse::Endorsement)> = vec![
                        (
                            "signed by the runtime's own key",
                            "a valid signature from the same party that wrote the rules",
                            endorse::sign(&used, &agent, scope, None, "", &runtime_key),
                        ),
                        (
                            "over a different policy",
                            "an endorsement of rules that are not these",
                            endorse::sign(&other_policy, &agent, scope, None, "", &endorser),
                        ),
                        (
                            "naming a different runtime",
                            "these rules, but accepted for somebody else",
                            endorse::sign(&used, "00ff00ff", scope, None, "", &endorser),
                        ),
                        (
                            "already expired",
                            "a spent endorsement, however well signed",
                            endorse::sign(&used, &agent, scope, Some(1), "", &endorser),
                        ),
                    ];
                    println!("\n  the endorsement, and what it must not accept:");
                    for (n, (what, why, e)) in cases.into_iter().enumerate() {
                        let v = serde_json::to_value(&e)?;
                        std::fs::write(
                            format!("{dir}/endorse-{n}.json"),
                            serde_json::to_string_pretty(&v)?,
                        )?;
                        println!("  {what}\n      — {why}");
                        match endorse::check(&v, &used, &agent, endorse::now_ms()) {
                            Ok(_) => println!("      → NOT CAUGHT — it was accepted"),
                            Err(reason) => println!("      → rejected: {reason}"),
                        }
                    }
                }
                _ => println!("\n  (the endorsement cases need both keys present)"),
            }
        }

        println!("\n  and the original, checked against its own published rules, still verifies");
    }
    Ok(())
}

async fn run_with_model(
    cdp: &mut Cdp,
    goal: &str,
    category: &str,
    max_steps: usize,
) -> Result<Run> {
    // Every run of this engine gets its own gate, made here rather than passed in, so there is no
    // call path that can reach an action without one.
    let run_host = skill::host_of_url(&cdp.current_url().await.unwrap_or_default());
    let mut gate = gate::Gate::new(&run_host);
    sign_for(&mut gate, &run_host);
    let started = Instant::now();
    let mut calls = 0u32;
    let mut trail: Vec<(Step, Element)> = Vec::new();
    let mut screens: Vec<Vec<Element>> = Vec::new();
    let mut log: Vec<String> = Vec::new();
    let mut declared_done = false;
    // Pages already visited. A run that comes back to one of them has stopped making
    // progress: everything after that point is wandering, and a skill compiled from it
    // would encode the wandering. The productive prefix ends at the first reversal.
    let mut seen: Vec<String> = vec![state::page_kind(&cdp.current_url().await?)];

    for _ in 0..max_steps {
        let (els, here) = cdp.state().await?;
        screens.push(els.clone());
        let d = match decide::next_step(goal, &here, &els, &log, category) {
            Ok(d) => d,
            Err(e) => {
                println!("  no decision came back: {e}");
                break;
            }
        };
        calls += 1;
        let Some(idx) = d.idx else {
            println!("   model: goal reached ({})", d.intent);
            declared_done = true;
            break;
        };
        let mut el = els[idx].clone();
        let action = match (&d.text, is_text_field(&el.role)) {
            (Some(text), true) => Action::Type { text: text.clone() },
            _ => Action::Click,
        };
        let mut step = skill::step_from(&el, action.clone(), &d.intent, d.confidence);
        step.learned_page = state::page_kind(&here);
        println!(
            "  step {}: {} «{}» — {} ({:.1} ms of model)",
            trail.len() + 1,
            el.role,
            el.name.chars().take(46).collect::<String>(),
            d.intent,
            d.latency_us as f64 / 1000.0
        );
        let (mut acted, blocked) = perform(cdp, &mut gate, &step, trail.len(), &el, None).await?;
        if let Some(why) = blocked {
            log.push(format!("the gate did not allow this step — {why}"));
            break;
        }
        if !acted {
            // The call failed, but the world may disagree: a click can land while the handle
            // it was aimed through has already gone stale, and then the page moves on before
            // the call comes back. Judge the action by what happened to the page, never by
            // the return value alone — otherwise a step that plainly worked is thrown away.
            cdp.settle(1500).await?;
            let now_page = state::page_kind(&cdp.current_url().await?);
            if now_page != step.learned_page {
                println!("  the handle was stale but the page moved to «{now_page}» — step accepted");
                acted = true;
            } else {
                println!("  the element vanished before the click — looking again");
                let (els2, _here2) = cdp.state().await?;
                let again = els2.iter().position(|e| {
                    e.role == el.role
                        && ((!el.href.is_empty() && e.href == el.href)
                            || state::word_score(e, &el.name) >= 0.5)
                });
                match again {
                    Some(k) => {
                        el = els2[k].clone();
                        let (again_ok, again_blocked) =
                            perform(cdp, &mut gate, &step, trail.len(), &el, None).await?;
                        acted = again_ok;
                        if let Some(why) = again_blocked {
                            // A retry is a second action. If the gate refuses it, the refusal is
                            // the answer — retrying around the gate is how a gate stops meaning
                            // anything.
                            log.push(format!("the gate did not allow the retry — {why}"));
                            break;
                        }
                    }
                    None => acted = false,
                }
            }
        }
        if !acted {
            println!("  the step did not happen and could not be retried — stopping");
            break;
        }
        cdp.settle(3000).await?;
        step.produced_page = state::page_kind(&cdp.current_url().await?);
        // A click that changed nothing is a miss, not progress backwards. Tell the model
        // so it can pick differently, and do not keep the step.
        if step.produced_page == step.learned_page {
            println!("  empty move: clicking «{}» changed nothing — telling the model and carrying on", el.name.chars().take(30).collect::<String>());
            log.push(format!(
                "failed: clicking {} «{}» led to no other page",
                el.role,
                el.name.chars().take(40).collect::<String>()
            ));
            continue;
        }
        if seen.contains(&step.produced_page) {
            println!("  reversal: the page «{}» was already visited — step dropped, the run is cut here", step.produced_page);
            break;
        }
        seen.push(step.produced_page.clone());
        log.push(format!("{} «{}»", el.role, el.name.chars().take(40).collect::<String>()));
        trail.push((step, el));
    }
    Ok(Run {
        ms: started.elapsed().as_millis() as u64,
        calls,
        trail,
        screens,
        declared_done,
    })
}

struct Run {
    ms: u64,
    calls: u32,
    trail: Vec<(Step, Element)>,
    screens: Vec<Vec<Element>>,
    declared_done: bool,
}

struct FastRun {
    ms: u64,
    done: usize,
    escalated: usize,
    mismatched: usize,
    notes: Vec<String>,
}

/// The fast run: no model, unless the runtime catches itself. Every step reports where its
/// element came from; a step that produces a page other than the one it promised is
/// treated as a failure and repaired on the spot.
async fn run_skill(cdp: &mut Cdp, sk: &mut Skill, category: &str) -> Result<FastRun> {
    // Every run of this engine gets its own gate, made here rather than passed in, so there is no
    // call path that can reach an action without one.
    let run_host = skill::host_of_url(&cdp.current_url().await.unwrap_or_default());
    let mut gate = gate::Gate::new(&run_host);
    sign_for(&mut gate, &run_host);
    let started = Instant::now();
    let mut notes = Vec::new();
    let mut done = 0usize;
    let mut escalated = 0usize;
    let mut mismatched = 0usize;
    let mut log: Vec<String> = Vec::new();
    let goal = sk.goal.clone();

    let mut i = 0usize;
    while i < sk.steps.len() {
        let t = Instant::now();
        let (els, here) = cdp.state().await?;
        let observe_us = t.elapsed().as_micros() as u64;
        let step = sk.steps[i].clone();

        let (el, how, resolve_us) = match {
            let t = Instant::now();
            let r = sk.resolve(&step, &els);
            (r, t.elapsed().as_micros() as u64)
        } {
            (Some((idx, _conf, how)), us) => {
                let el = els[idx].clone();
                notes.push(format!(
                    "  step {}: {} «{}» — recognised by {} ({:.2} ms of decision)",
                    i + 1,
                    el.role,
                    el.name.chars().take(44).collect::<String>(),
                    how.as_str(),
                    us as f64 / 1000.0
                ));
                (el, how, us)
            }
            (None, us) => {
                notes.push(format!("  step {}: not recognised — asking the model", i + 1));
                escalated += 1;
                match recover(cdp, &goal, category, &mut log).await? {
                    Some(el) => (el, Found::Taught, us),
                    None => {
                        notes.push("  the model found no move either — stopping".into());
                        break;
                    }
                }
            }
        };
        let _ = observe_us;
        let _ = resolve_us;

        let (acted, blocked) = perform(cdp, &mut gate, &step, i, &el, None).await?;
        if let Some(why) = blocked {
            notes.push(format!("  step {}: the gate did not allow it — {}", i + 1, why));
            break;
        }
        if !acted {
            cdp.settle(2500).await?;
            let (els2, _h) = cdp.state().await?;
            match sk.resolve(&sk.steps[i], &els2) {
                Some((idx, _c, how2)) => {
                    let el2 = els2[idx].clone();
                    let _ = perform(cdp, &mut gate, &step, i, &el2, None).await?;
                    notes.push(format!("  step {}: the element moved, but was found again by {}", i + 1, how2.as_str()));
                }
                None => {
                    notes.push(format!("  step {}: the element vanished and was not found again", i + 1));
                    break;
                }
            }
        }
        cdp.settle(3000).await?;
        let after = cdp.current_url().await?;

        let expected = sk.steps[i].produced_page.clone();
        if how != Found::Taught && !expected.is_empty() && state::page_kind(&after) != expected {
            mismatched += 1;
            notes.push(format!(
                "  step {} did not do what it promised (expected «{}», got «{}») — repairing",
                i + 1,
                expected,
                state::page_kind(&after)
            ));
            escalated += 1;
            if let Some(el2) = recover(cdp, &goal, category, &mut log).await? {
                cdp.settle(3000).await?;
                let healed_page = state::page_kind(&cdp.current_url().await?);
                let screen = cdp.state().await?.0;
                sk.teach(i, &el2, &screen, &here, &healed_page);
                notes.push(format!(
                    "  step {} re-taught to {} «{}»",
                    i + 1,
                    el2.role,
                    el2.name.chars().take(40).collect::<String>()
                ));
            }
        } else if how == Found::Taught {
            let healed_page = state::page_kind(&after);
            let screen = cdp.state().await?.0;
            sk.teach(i, &el, &screen, &here, &healed_page);
        }
        log.push(format!("{} «{}»", el.role, el.name.chars().take(40).collect::<String>()));
        done += 1;
        i += 1;
    }
    Ok(FastRun {
        ms: started.elapsed().as_millis() as u64,
        done,
        escalated,
        mismatched,
        notes,
    })
}

/// Ask the model for the next step, in the middle of a fast run.
async fn recover(cdp: &mut Cdp, goal: &str, category: &str, log: &mut Vec<String>) -> Result<Option<Element>> {
    // A recovery is a single step found by a model, so it gets a gate of its own. It is still a
    // decision written before the action — the same boundary, reached from a different place.
    let run_host = skill::host_of_url(&cdp.current_url().await.unwrap_or_default());
    let mut gate = gate::Gate::new(&run_host);
    sign_for(&mut gate, &run_host);
    let (els, here) = cdp.state().await?;
    let d = decide::next_step(goal, &here, &els, log, category)?;
    let Some(idx) = d.idx else { return Ok(None) };
    let el = els[idx].clone();
    let action = match (&d.text, is_text_field(&el.role)) {
        (Some(t), true) => Action::Type { text: t.clone() },
        _ => Action::Click,
    };
    let step = skill::step_from(&el, action.clone(), &d.intent, d.confidence);
    let (acted, blocked) = perform(cdp, &mut gate, &step, 0, &el, None).await?;
    if blocked.is_some() || !acted {
        return Ok(None);
    }
    log.push(format!("{} «{}»", el.role, el.name.chars().take(40).collect::<String>()));
    Ok(Some(el))
}

async fn learn(args: &[String]) -> Result<()> {
    let goal = flag(args, "--goal").ok_or_else(|| anyhow!("--goal is required"))?;
    let start = flag(args, "--start").ok_or_else(|| anyhow!("--start is required"))?;
    let out = flag(args, "--skill").unwrap_or_else(|| "skill.json".into());
    let max: usize = flag(args, "--max-steps").and_then(|v| v.parse().ok()).unwrap_or(8);
    let category = flag(args, "--category").unwrap_or_else(|| "browser".into());

    println!("FIRST RUN — the model decides every step\ngoal: {goal}\npage: {start}\n");
    if let Ok(closed) = Cdp::tidy_tabs(PORT).await {
        if closed > 0 {
            println!("  leftover tabs closed: {closed}\n");
        }
    }
    let mut cdp = Cdp::connect(PORT).await?;
    cdp.navigate(&start).await?;
    cdp.settle(4000).await?;
    let run = run_with_model(&mut cdp, &goal, &category, max).await?;
    let title = cdp.title().await?;
    // The kind of page the *kept* steps end on — not wherever the wandering run stopped.
    let final_kind = run
        .trail
        .last()
        .map(|(st, _)| st.produced_page.clone())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| state::page_kind(&start));
    let mut sk = skill::compile(&goal, &start, &run.trail, &run.screens, run.calls, run.ms, &final_kind);
    // A run that never declared success is not evidence that the skill works. Say so in
    // the file, so no later report can quietly claim otherwise.
    // Ask once whether the place we ended up actually satisfies the goal. The verdict is
    // about the outcome, not about our own recording of it.
    let body_text = cdp.text().await.unwrap_or_default();
    let here_now = cdp.current_url().await?;
    let verdict = decide::judge_success(&goal, &here_now, &title, &body_text);
    sk.learn_declared_done = match verdict {
        Ok((ok, why)) => {
            println!("goal check: {} — {why}", if ok { "reached" } else { "NOT reached" });
            ok
        }
        Err(e) => {
            println!("the goal check failed ({e}) — the skill is treated as unverified");
            false
        }
    };
    sk.save(&out)?;
    println!(
        "\nresult: {} steps, {} model calls, {:.1} s",
        run.trail.len(),
        run.calls,
        run.ms as f64 / 1000.0
    );
    if !sk.learn_declared_done {
        println!("WARNING: this skill is marked unverified — do not present its success as proven");
    } else if !run.declared_done {
        println!("the run was cut at a reversal, but the goal is confirmed — the step that reached it is kept");
    }
    println!("landed on: {title}");
    println!("the experience is written down as a program: {out}");
    Ok(())
}

async fn race(args: &[String]) -> Result<()> {
    let path = flag(args, "--skill").ok_or_else(|| anyhow!("--skill is required"))?;
    let target = flag(args, "--start").ok_or_else(|| anyhow!("--start is required"))?;
    let max: usize = flag(args, "--max-steps").and_then(|v| v.parse().ok()).unwrap_or(8);
    let category = flag(args, "--category").unwrap_or_else(|| "browser".into());
    let mut sk = Skill::load(&path)?;

    println!("NEW DATA, THE SAME SKILL\n");
    println!("learned on: {}", sk.learned_from);
    println!("what it cost back then: {} model calls, {:.1} s", sk.learn_model_calls, sk.learn_ms as f64 / 1000.0);
    println!(
        "the goal check confirmed success: {}\n",
        if sk.learn_declared_done { "yes" } else { "NO — the skill is unverified" }
    );

    let mut slow = Cdp::connect(PORT).await?;
    slow.navigate(&target).await?;
    slow.settle(4000).await?;
    println!("THE OLD PATH — the model decides every step:");
    let slow_run = run_with_model(&mut slow, &sk.goal, &category, max).await?;
    let slow_url = slow.current_url().await?;
    let slow_title = slow.title().await?;
    println!(
        "  → {:.1} s, {} model calls, {} steps → {}",
        slow_run.ms as f64 / 1000.0,
        slow_run.calls,
        slow_run.trail.len(),
        trim(&slow_title)
    );

    let mut fast = Cdp::connect(PORT).await?;
    fast.navigate(&target).await?;
    fast.settle(4000).await?;
    println!("\nMINT — running the remembered program:");
    let r = run_skill(&mut fast, &mut sk, &category).await?;
    for n in &r.notes {
        println!("{n}");
    }
    let fast_url = fast.current_url().await?;
    let fast_title = fast.title().await?;
    let reached = state::page_kind(&fast_url) == sk.expect_final_page;
    println!("  → {:.2} s, {} model calls, {} steps → {}", r.ms as f64 / 1000.0, r.escalated, r.done, trim(&fast_title));
    println!("  reached the goal: {}", if reached { "yes" } else { "no — it landed somewhere else, and that is shown rather than hidden" });

    if r.escalated > 0 {
        let healed = format!("{path}.healed.json");
        sk.save(&healed)?;
        println!("  the program was repaired and saved: {healed}");
    }

    println!("\nSUMMARY");
    println!("  the old path: {:.1} s, {} model calls → {slow_url}", slow_run.ms as f64 / 1000.0, slow_run.calls);
    println!("  mint:       {:.2} s, {} model calls, {} self-repairs → {fast_url}", r.ms as f64 / 1000.0, r.escalated, r.mismatched);
    println!("  faster by {:.1}x", slow_run.ms as f64 / (r.ms as f64).max(1.0));
    Ok(())
}


/// Wait until the page reaches the kind a step promised, rather than for a fixed pause.
/// Falls back to plain settling when the promised kind never appears, so a wrong
/// expectation slows the run down instead of hanging it.
async fn settle_until(cdp: &mut Cdp, expected: &str, max_ms: u64) -> Result<()> {
    if expected.is_empty() {
        return cdp.settle(max_ms).await;
    }
    let deadline = Instant::now() + std::time::Duration::from_millis(max_ms);
    while Instant::now() < deadline {
        let url = cdp.current_url().await?;
        if state::page_kind(&url) == expected {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    cdp.settle(1200).await
}

/// Send one event to the showcase page. Failures are ignored on purpose: the page is a
/// window onto the run, and a window that breaks must never break the run itself.
fn push_event(relay: &str, path: &str, payload: serde_json::Value) {
    let _ = ureq::post(&format!("{relay}{path}"))
        .set("Content-Type", "application/json")
        .send_json(payload);
}

/// Exercise the gate on real elements and print what it decided, and why.
///
/// Four elements, four kinds of answer: a navigation is allowed, a field is allowed, a button is
/// held until an approval bound to that exact action arrives, and an element that is neither is
/// refused. Then the same approval is presented a second time and refused as a repeat, because the
/// one thing a deterministic replayer must never do is the same effect twice.
///
/// The refusals are not taken on trust: the page itself reports whether anything happened, and the
/// report says what the page said afterwards.
async fn gate_demo(args: &[String]) -> Result<()> {
    // The showcase port, not the debugging port: `PORT` above is the browser's.
    let url = flag(args, "--url").unwrap_or_else(|| "http://127.0.0.1:8840/gate-lab".to_string());
    let out_path = flag(args, "--out").unwrap_or_else(|| "demo/gate-report.json".into());
    // Everything this demonstration writes follows --out, so that a checkout on somebody else's
    // machine writes into the directory they chose rather than into a path of ours that may not exist.
    let dir = std::path::Path::new(&out_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".into());
    let receipts_path = format!("{dir}/gate-receipts.json");
    let jsonl_path = format!("{dir}/gate-receipts.jsonl");
    let policy_path = format!("{dir}/gate-policy.json");
    let endorsement_path =
        flag(args, "--endorsement").unwrap_or_else(|| format!("{dir}/gate-endorsement.json"));

    if let Ok(closed) = Cdp::tidy_tabs(PORT).await {
        if closed > 0 {
            println!("  leftover tabs closed: {closed}");
        }
    }
    let mut cdp = Cdp::connect(PORT).await?;
    cdp.navigate(&url).await?;
    cdp.settle(4000).await?;
    let (els, here) = cdp.state().await?;
    println!("  page: {here} — {} elements on it\n", els.len());

    let pick = |tag: &str, href: bool| -> Option<Element> {
        els.iter()
            .find(|e| e.tag == tag && (!href || !e.href.trim().is_empty()))
            .cloned()
    };
    let link = pick("a", true);
    let field = pick("input", false);
    let button = pick("button", false);
    let odd = els.iter().find(|e| e.role == "tab").cloned();

    let host = skill::host_of_url(&url);
    let mut gate = gate::Gate::new(&host);
    sign_for(&mut gate, &host);

    let cases: Vec<(&str, Option<Element>, Action, &str)> = vec![
        ("navigation", link, Action::Click, "open the record"),
        ("field", field, Action::Type { text: "REF-1".into() }, "fill the reference"),
        ("button", button.clone(), Action::Click, "submit the filing"),
        ("unclassifiable", odd.clone(), Action::Click, "press the thing that is neither"),
    ];

    let mut steps: Vec<(Step, Element)> = Vec::new();
    println!("  {:<16} {:<9} {:<8} {}", "element", "would do", "verdict", "why");
    for (i, (label, el, action, intent)) in cases.iter().enumerate() {
        let Some(el) = el.clone() else {
            println!("  {label:<16} — not on the page, skipped");
            continue;
        };
        let step = skill::step_from(&el, action.clone(), intent, 1.0);
        let class = gate::classify(&step.action, &el);
        let (verdict, _key) = gate.decide(&step, i, &el, None);
        println!(
            "  {:<16} {:<9} {:<8} {}",
            label,
            class.as_str(),
            verdict.as_str(),
            if verdict.reason().is_empty() { "safe to perform" } else { verdict.reason() }
        );
        steps.push((step, el));
    }

    // The button again, this time with an approval bound to its exact identity.
    let mut approved_index: Option<usize> = None;
    if let Some(index) = steps.iter().position(|(_, e)| e.tag == "button") {
        let (step, el) = steps[index].clone();
        let identity = gate::action_identity(&host, &step, index);
        let token = gate::approval_for(&identity);
        println!(
            "\n  {:<16} {:<9} approval {}{}",
            "button, approved", "write", &identity[..12],
            " presented — this is the only thing that lets it through"
        );

        // The write goes through the same door as everything else: with the approval bound to this
        // exact action, which is what the gate is waiting for.
        let (done, blocked) = perform(&mut cdp, &mut gate, &step, index, &el, Some(&token)).await?;
        if let Some(why) = blocked {
            println!("  {:<16} {:<9} not performed — {why}", "button, approved", "write");
        } else if done {
            approved_index = Some(index);
            cdp.settle(1200).await?;
            let after = cdp.text().await.unwrap_or_default();
            let line = after
                .lines()
                .find(|l| l.contains("committed") || l.contains("unclassifiable element ran"))
                .unwrap_or("the page did not report anything")
                .trim()
                .to_string();
            println!("  {:<16} {:<9} the page now says: {line}", "performed", "write");
        }

        // And now the same approval, offered a second time, after the effect has happened.
        let (v2, _) = gate.decide(&step, index, &el, Some(&token));
        println!(
            "  {:<16} {:<9} {:<8} {}",
            "the same again", "write", v2.as_str(), v2.reason()
        );
    }

    // And what a person must be able to check afterwards: did anything run that should not have?
    let final_text = cdp.text().await.unwrap_or_default();
    let odd_ran = final_text.contains("the unclassifiable element ran");
    let committed = final_text.contains("committed");
    println!(
        "\n  the unclassifiable element ran: {}",
        if odd_ran { "YES — the gate failed" } else { "no — it was refused and nothing happened" }
    );
    println!("  the approved write happened:    {}", if committed { "yes, once" } else { "no" });

    let (allow, hold, refuse) = gate.tally();
    let report = serde_json::json!({
        "collected_at": now_stamp(),
        "url": url,
        "allowed": allow, "held": hold, "refused": refuse,
        "unclassifiable_element_ran": odd_ran,
        "approved_write_happened": committed,
        "approved_step": approved_index,
        "log": gate.report(),
    });
    std::fs::write(&out_path, serde_json::to_string_pretty(&report)?)?;

    // The counterparty's endorsement, if one has been made. The runtime does not mint one: it can
    // only carry a signature that already exists, which is the difference between a counter-
    // signature and a self-portrait. And it checks it before carrying it, so a log never carries an
    // endorsement that does not hold for this policy and this runtime.
    match std::fs::read_to_string(&endorsement_path) {
        Ok(text) => match (serde_json::from_str::<serde_json::Value>(&text), gate.receipt_log_mut()) {
            (Ok(v), Some(log)) => {
                let rules_here = log.rules.clone();
                let agent_here = log.agent.clone();
                match endorse::check(&v, &rules_here, &agent_here, endorse::now_ms()) {
                    Ok(e) => {
                        println!(
                            "\n  carrying the endorsement by {} — a different key from the runtime's",
                            &e.body.endorser[..12.min(e.body.endorser.len())]
                        );
                        log.accept_endorsement(&e);
                    }
                    Err(why) => println!(
                        "\n  an endorsement file exists but does not hold for this log: {why}"
                    ),
                }
            }
            _ => println!("\n  an endorsement file exists but does not parse"),
        },
        Err(_) => println!(
            "\n  no endorsement on file: nobody outside this runtime has signed these rules"
        ),
    }

    // The signed record, in both shapes: one document, and one line per receipt.
    if let Some(log) = gate.receipt_log() {
        let doc = log.document();
        std::fs::write(&receipts_path, serde_json::to_string_pretty(&doc)?)?;
        let mut lines = String::new();
        for r in log.receipts() {
            lines.push_str(&serde_json::to_string(r)?);
            lines.push('\n');
        }
        std::fs::write(&jsonl_path, lines)?;
        // The rules themselves, published next to the log. A hash with no document behind it proves
        // nothing, and the point of signing the policy is that somebody can read what they are
        // being asked to accept.
        std::fs::write(&policy_path, serde_json::to_string_pretty(&log.rules)?)?;
        let v = receipt::verify(&doc, Some(&log.rules));
        println!(
            "\n  signed {} receipts as {} under policy {}",
            log.len(),
            &log.agent[..12],
            &log.policy_hash[..12]
        );
        println!(
            "  verification: {}",
            if v.ok {
                format!("all {} hold, and the rules reproduce all {} verdicts", v.checked, v.replayed)
            } else {
                "BROKEN".to_string()
            }
        );
    }
    println!("\n  {allow} allowed, {hold} held, {refuse} refused — log: {out_path}");
    Ok(())
}

/// Both runs, streamed to the showcase page: the old way first, so the audience sees the
/// thinking it is about to stop paying for, then the compiled way.
async fn demo(args: &[String]) -> Result<()> {
    let path = flag(args, "--skill").ok_or_else(|| anyhow!("--skill is required"))?;
    let target = flag(args, "--start").ok_or_else(|| anyhow!("--start is required"))?;
    // One gate for the whole demonstration, so a repeat inside the run is seen as a repeat.
    let demo_host = skill::host_of_url(&target);
    let mut gate = gate::Gate::new(&demo_host);
    sign_for(&mut gate, &demo_host);
    let relay = flag(args, "--push").unwrap_or_else(|| "http://127.0.0.1:8840".into());
    let max: usize = flag(args, "--max-steps").and_then(|v| v.parse().ok()).unwrap_or(6);
    let category = flag(args, "--category").unwrap_or_else(|| "browser".into());
    let sk = Skill::load(&path)?;
    let goal = sk.goal.clone();

    push_event(&relay, "/reset", json!({ "goal": goal }));
    println!("showcase: {relay} | goal: {goal}");

    // ── side 1: the model decides every step ────────────────────────────────────
    let mut slow = Cdp::connect(PORT).await?;
    slow.navigate(&target).await?;
    slow.settle(4000).await?;
    let slow_started = Instant::now();
    let mut log: Vec<String> = Vec::new();
    let mut n = 0usize;
    println!("\nORDINARY AGENT");
    for _ in 0..max {
        let (els, here) = slow.state().await?;
        let d = match decide::next_step(&goal, &here, &els, &log, &category) {
            Ok(d) => d,
            Err(e) => {
                println!("  no decision came back: {e}");
                break;
            }
        };
        let Some(idx) = d.idx else {
            println!("  model: goal reached");
            break;
        };
        let el = els[idx].clone();
        let action = match (&d.text, is_text_field(&el.role)) {
            (Some(t), true) => Action::Type { text: t.clone() },
            _ => Action::Click,
        };
        n += 1;
        println!("  step {n}: {} «{}» — {:.0} ms of model", el.role, el.name.chars().take(40).collect::<String>(), d.latency_us as f64 / 1000.0);
        // Push before acting: every decision the model was paid for has to appear in the
        // counter, including one that turns out to be unactionable a moment later.
        let frame_before = slow.screenshot(55).await.unwrap_or_default();
        push_event(&relay, "/push", json!({
            "side": "slow",
            "step": { "n": n, "element": format!("{} «{}»", el.role, el.name.chars().take(40).collect::<String>()),
                      "tier": "model", "decision_us": d.latency_us, "note": d.intent },
            "frame": frame_before,
            "total_ms": slow_started.elapsed().as_millis() as u64,
            "url": slow.current_url().await?,
        }));
        let slow_step = skill::step_from(&el, action.clone(), &d.intent, d.confidence);
        if !perform(&mut slow, &mut gate, &slow_step, n, &el, None).await?.0 {
            break;
        }
        slow.settle(3000).await?;
        log.push(format!("{} «{}»", el.role, el.name.chars().take(40).collect::<String>()));
    }
    let slow_ms = slow_started.elapsed().as_millis() as u64;
    let slow_url = slow.current_url().await?;
    let slow_title = slow.title().await?;
    let slow_body = slow.text().await.unwrap_or_default();
    // The verdict is a check, not an opinion: the page we reached has to be of the kind
    // the learning run recorded. What a model thinks about it is shown separately, and
    // labelled as an opinion, because that is all it is.
    let slow_kind = state::page_kind(&slow_url);
    let slow_ok = slow_kind == sk.expect_final_page;
    let slow_verdict = format!(
        "landed on page «{}», expected «{}» — {}",
        slow_kind,
        sk.expect_final_page,
        if slow_ok { "matches" } else { "does NOT match" }
    );
    let slow_opinion = match decide::judge_success(&goal, &slow_url, &slow_title, &slow_body) {
        Ok((ok, why)) => format!("{}: {why}", if ok { "says it was reached" } else { "says it was not reached" }),
        Err(_) => String::new(),
    };
    println!("  → {:.1} s, result: {slow_verdict} | the model says: {slow_opinion}", slow_ms as f64 / 1000.0);
    push_event(&relay, "/push", json!({ "side": "slow", "finished": true, "total_ms": slow_ms,
        "url": slow_url, "page_title": slow_title, "verdict": slow_verdict, "opinion": slow_opinion }));

    // ── side 2: the remembered skill runs, in a second tab ─────────────
    println!("\nMINT");
    let ws = Cdp::open_tab(PORT, "about:blank").await?;
    let mut fast = Cdp::connect_ws(&ws).await?;
    fast.navigate(&target).await?;
    fast.settle(4000).await?;
    let fast_started = Instant::now();
    let mut done = 0usize;
    let mut escalated = 0usize;
    for i in 0..sk.steps.len() {
        let (els, _here) = fast.state().await?;
        let step = sk.steps[i].clone();
        let t = Instant::now();
        let found = sk.resolve(&step, &els);
        let decision_us = t.elapsed().as_micros() as u64;
        let (el, tier, note) = match found {
            Some((idx, _c, how)) => (els[idx].clone(), "skill", format!("recognised by {}", how.as_str())),
            None => {
                escalated += 1;
                match recover(&mut fast, &goal, &category, &mut Vec::new()).await? {
                    Some(el) => (el, "model", "not recognised — asked the model".to_string()),
                    None => break,
                }
            }
        };
        done += 1;
        println!("  step {}: {} «{}» — {:.2} ms of decision ({note})", i + 1, el.role, el.name.chars().take(40).collect::<String>(), decision_us as f64 / 1000.0);
        let frame_before = fast.screenshot(55).await.unwrap_or_default();
        push_event(&relay, "/push", json!({
            "side": "fast",
            "step": { "n": i + 1, "element": format!("{} «{}»", el.role, el.name.chars().take(40).collect::<String>()),
                      "tier": tier, "decision_us": decision_us, "note": note },
            "frame": frame_before,
            "total_ms": fast_started.elapsed().as_millis() as u64,
            "url": fast.current_url().await?,
        }));
        if !perform(&mut fast, &mut gate, &step, i, &el, None).await?.0 {
            break;
        }
        // A compiled step knows the page kind it should produce, so it can stop waiting the
        // moment that page appears instead of sitting out a fixed pause. Knowing the
        // outcome is what makes the fast side fast on the clock too, not only in thinking.
        settle_until(&mut fast, &step.produced_page, 2500).await?;
        let frame = fast.screenshot(55).await.unwrap_or_default();
        push_event(&relay, "/push", json!({ "side": "fast", "frame": frame }));
    }
    let fast_ms = fast_started.elapsed().as_millis() as u64;
    let fast_url = fast.current_url().await?;
    let fast_title = fast.title().await?;
    let fast_body = fast.text().await.unwrap_or_default();
    let fast_kind = state::page_kind(&fast_url);
    let fast_ok = fast_kind == sk.expect_final_page;
    let fast_verdict = format!(
        "landed on page «{}», expected «{}» — {}",
        fast_kind,
        sk.expect_final_page,
        if fast_ok { "matches" } else { "does NOT match" }
    );
    let fast_opinion = match decide::judge_success(&goal, &fast_url, &fast_title, &fast_body) {
        Ok((ok, why)) => format!("{}: {why}", if ok { "says it was reached" } else { "says it was not reached" }),
        Err(_) => String::new(),
    };
    println!("  → {:.2} s, {escalated} model calls, result: {fast_verdict}", fast_ms as f64 / 1000.0);
    push_event(&relay, "/push", json!({ "side": "fast", "finished": true, "total_ms": fast_ms,
        "url": fast_url, "page_title": fast_title, "verdict": fast_verdict, "opinion": fast_opinion }));

    println!("\nTHE NUMBER FOR THE STAGE");
    println!("  ordinary agent: {:.1} s, {n} steps, every one a model call", slow_ms as f64 / 1000.0);
    println!("  mint:         {:.2} s, {done} steps, model calls {escalated}", fast_ms as f64 / 1000.0);
    Ok(())
}


/// Run one already-learned task on a live page, in front of whoever is watching.
///
/// This is the working half of `demo` on its own: no second pane, no race, and no model unless a step
/// cannot be recognised at all. It exists for the demonstration page, where somebody presses a button
/// and the runtime goes and does the thing while they watch — the frames of the real browser, every
/// step with the tier that resolved it, and a signed receipt for each decision, including the one the
/// gate refused.
async fn live(args: &[String]) -> Result<()> {
    let path = flag(args, "--skill").ok_or_else(|| anyhow!("--skill is required"))?;
    let target = flag(args, "--start").ok_or_else(|| anyhow!("--start is required"))?;
    let relay = flag(args, "--push").unwrap_or_else(|| "http://127.0.0.1:8840".into());
    let out = flag(args, "--receipts").unwrap_or_else(|| "live-run.json".into());
    let category = flag(args, "--category").unwrap_or_else(|| "browser".into());

    let sk = Skill::load(&path)?;
    let goal = sk.goal.clone();
    let host = skill::host_of_url(&target);
    let mut gate = gate::Gate::new(&host);
    sign_for(&mut gate, &host);
    push_event(&relay, "/live/reset", json!({ "goal": goal, "skill": path }));
    println!("live: {goal}");

    let mut page = Cdp::connect(PORT).await?;
    page.navigate(&target).await?;
    page.settle(3500).await?;
    let started = Instant::now();
    let (mut done, mut asked, mut held) = (0usize, 0usize, 0usize);

    for i in 0..sk.steps.len() {
        let (els, _here) = page.state().await?;
        let step = sk.steps[i].clone();
        let t = Instant::now();
        let (el, tier, note) = match sk.resolve(&step, &els) {
            Some((idx, _c, how)) => (
                els[idx].clone(),
                "skill",
                format!("recognised by {}", how.as_str()),
            ),
            None => {
                asked += 1;
                match recover(&mut page, &goal, &category, &mut Vec::new()).await? {
                    Some(el) => (el, "model", "not recognised — asked the model".to_string()),
                    None => break,
                }
            }
        };
        let decision_us = t.elapsed().as_micros() as u64;
        done += 1;
        let described = format!("{} «{}»", el.role, el.name.chars().take(40).collect::<String>());
        let frame = page.screenshot(55).await.unwrap_or_default();
        push_event(
            &relay,
            "/push",
            json!({
                "side": "live",
                "step": { "n": i + 1, "element": described, "tier": tier,
                          "decision_us": decision_us, "note": note },
                "frame": frame,
                "url": page.current_url().await?,
                "total_ms": started.elapsed().as_millis() as u64,
            }),
        );

        // The gate decides here, as it does everywhere else. A step it refuses is not performed, and
        // the refusal is what the viewer sees — which is the demonstration, not an interruption.
        let (performed, why) = perform(&mut page, &mut gate, &step, i, &el, None).await?;
        if !performed {
            held += 1;
            push_event(
                &relay,
                "/push",
                json!({ "side": "live", "step": { "n": i + 1, "element": described,
                        "tier": "gate", "decision_us": 0,
                        "note": why.unwrap_or_else(|| "not performed".into()) } }),
            );
            break;
        }
        settle_until(&mut page, &step.produced_page, 2500).await?;
        let frame = page.screenshot(55).await.unwrap_or_default();
        push_event(&relay, "/push", json!({ "side": "live", "frame": frame }));
    }

    let ms = started.elapsed().as_millis() as u64;
    let landed = page.current_url().await?;
    let kind = state::page_kind(&landed);
    let ok = kind == sk.expect_final_page;
    let verdict = format!(
        "landed on page «{}», expected «{}» — {}",
        kind,
        sk.expect_final_page,
        if ok { "matches" } else { "does NOT match" }
    );

    let mut receipts = json!({ "signed": false, "reason": "signing was unavailable for this run" });
    if let Some(log) = gate.receipt_log() {
        let doc = log.document();
        std::fs::write(&out, serde_json::to_string_pretty(&doc)?)?;
        let rules_file = format!("{out}.policy.json");
        std::fs::write(&rules_file, serde_json::to_string_pretty(&log.rules)?)?;
        let v = receipt::verify(&doc, Some(&log.rules));
        receipts = json!({
            "signed": true,
            "count": log.len(),
            "verified": v.ok,
            "file": out,
            "policy_file": rules_file,
        });
    }
    println!("  {done} steps, {asked} asked, {held} held | {verdict}");
    push_event(
        &relay,
        "/push",
        json!({ "side": "live", "finished": true, "total_ms": ms, "url": landed,
                "verdict": verdict, "receipts": receipts,
                "steps_done": done, "asked": asked, "held": held }),
    );
    Ok(())
}

#[derive(serde::Deserialize)]
struct Job {
    skill: String,
    start: String,
}

/// Many tasks in a row: the counter has to show a number of decisions big enough that the
/// difference stops being a detail. Each job starts on a different page, and only the
/// first task of each kind was ever learned — the rest are transfers.
async fn chain(args: &[String]) -> Result<()> {
    let jobs_path = flag(args, "--jobs").ok_or_else(|| anyhow!("--jobs is required"))?;
    let relay = flag(args, "--push").unwrap_or_else(|| "http://127.0.0.1:8840".into());
    let max: usize = flag(args, "--max-steps").and_then(|v| v.parse().ok()).unwrap_or(3);
    let category = flag(args, "--category").unwrap_or_else(|| "project sources".into());
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&jobs_path)?)?;
    let slow_host = skill::host_of_url(
        &jobs.first().map(|j| j.start.clone()).unwrap_or_default(),
    );
    let mut slow_gate = gate::Gate::new(&slow_host);
    let total = jobs.len();

    // Start from a known tab. Leftover tabs from earlier runs are how a working run ends up
    // attached to a stale page and looks broken.
    if let Ok(closed) = Cdp::tidy_tabs(PORT).await {
        if closed > 0 {
            println!("  leftover tabs closed before the run: {closed}");
        }
    }

    push_event(&relay, "/reset", json!({
        "goal": format!("{total} repository tasks in a row: the latest commit and the issue list across different projects")
    }));
    println!("showcase: {relay} | tasks: {total}\n");

    // ── THE OLD PATH ────────────────────────────────────────────────────────────
    println!("ORDINARY AGENT — the model at every step");
    let mut slow = Cdp::connect(PORT).await?;
    let slow_started = Instant::now();
    let mut slow_steps = 0usize;
    let mut slow_dec_us = 0u64;
    for (ji, job) in jobs.iter().enumerate() {
        let sk = Skill::load(&job.skill)?;
        slow.navigate(&job.start).await?;
        slow.settle(4000).await?;
        let mut log: Vec<String> = Vec::new();
        for _ in 0..max {
            let (els, here) = slow.state().await?;
            let d = match decide::next_step(&sk.goal, &here, &els, &log, &category) {
                Ok(d) => d,
                Err(e) => {
                    println!("  task {}/{total}: no decision came back ({e})", ji + 1);
                    break;
                }
            };
            let Some(idx) = d.idx else { break };
            let el = els[idx].clone();
            let action = match (&d.text, is_text_field(&el.role)) {
                (Some(t), true) => Action::Type { text: t.clone() },
                _ => Action::Click,
            };
            slow_steps += 1;
            slow_dec_us += d.latency_us;
            println!("  task {}/{total}, step {slow_steps}: {} «{}» — {:.0} ms of model",
                ji + 1, el.role, el.name.chars().take(34).collect::<String>(), d.latency_us as f64 / 1000.0);
            let frame_before = slow.screenshot(50).await.unwrap_or_default();
            push_event(&relay, "/push", json!({
                "side": "slow",
                "step": { "n": slow_steps,
                          "element": format!("[{}] {} «{}»", ji + 1, el.role, el.name.chars().take(34).collect::<String>()),
                          "tier": "model", "decision_us": d.latency_us, "note": d.intent },
                "frame": frame_before,
                "total_ms": slow_started.elapsed().as_millis() as u64,
                "url": slow.current_url().await?,
            }));
            let slow_step = skill::step_from(&el, action.clone(), &d.intent, d.confidence);
            if !perform(&mut slow, &mut slow_gate, &slow_step, ji, &el, None).await?.0 { break; }
            slow.settle(3000).await?;
            log.push(format!("{} «{}»", el.role, el.name.chars().take(34).collect::<String>()));
        }
    }
    let slow_ms = slow_started.elapsed().as_millis() as u64;
    let slow_url = slow.current_url().await?;
    let slow_verdict = format!("{slow_steps} decisions, every one of them a model call");
    println!("  → {:.1} s, {slow_steps} decisions\n", slow_ms as f64 / 1000.0);
    push_event(&relay, "/push", json!({ "side": "slow", "finished": true, "total_ms": slow_ms,
        "url": slow_url, "verdict": slow_verdict }));

    // ── THE COMPILED PATH ──────────────────────────────────────────────────
    println!("MINT — from memory, on the same pages with different data");
    let ws = Cdp::open_tab(PORT, "about:blank").await?;
    let mut fast = Cdp::connect_ws(&ws).await?;
    let fast_started = Instant::now();
    let mut fast_steps = 0usize;
    let mut fast_dec_us = 0u64;
    let mut escalated = 0usize;
    let mut corrections = 0usize;
    for (ji, job) in jobs.iter().enumerate() {
        let mut sk = Skill::load(&job.skill)?;
        let mut gate = gate::Gate::new(&sk.host);
        fast.navigate(&job.start).await?;
        fast.settle(4000).await?;
        let mut log: Vec<String> = Vec::new();
        for i in 0..sk.steps.len() {
            let (els, _here) = fast.state().await?;
            let step = sk.steps[i].clone();
            let t = Instant::now();
            let found = sk.resolve(&step, &els);
            let decision_us = t.elapsed().as_micros() as u64;
            let (el, tier, note) = match found {
                Some((idx, _c, how)) => (els[idx].clone(), "skill", format!("recognised by {}", how.as_str())),
                None => {
                    escalated += 1;
                    // The compiled path must not die because the model is unavailable: working
                    // without it is the whole point. If a step needs a question and the question
                    // cannot be asked, that job is skipped and the chain carries on.
                    match recover(&mut fast, &sk.goal, &category, &mut log).await {
                        Ok(Some(el)) => (el, "model", "not recognised — asked the model".to_string()),
                        Ok(None) => break,
                        Err(e) => {
                            println!("  the model is unavailable ({e}) — task skipped, the chain carries on");
                            break;
                        }
                    }
                }
            };
            fast_steps += 1;
            fast_dec_us += decision_us;
            println!("  task {}/{total}, step {fast_steps}: {} «{}» — {:.2} ms ({note})",
                ji + 1, el.role, el.name.chars().take(34).collect::<String>(), decision_us as f64 / 1000.0);
            let frame_before = fast.screenshot(50).await.unwrap_or_default();
            push_event(&relay, "/push", json!({
                "side": "fast",
                "step": { "n": fast_steps,
                          "element": format!("[{}] {} «{}»", ji + 1, el.role, el.name.chars().take(34).collect::<String>()),
                          "tier": tier, "decision_us": decision_us, "note": note },
                "frame": frame_before,
                "total_ms": fast_started.elapsed().as_millis() as u64,
                "url": fast.current_url().await?,
            }));
            if !perform(&mut fast, &mut gate, &step, i, &el, None).await?.0 {
                cdp_reobserve(&mut fast, &mut sk, i).await?;
                corrections += 1;
                continue;
            }
            settle_until(&mut fast, &step.produced_page, 2500).await?;
        }
    }
    let fast_ms = fast_started.elapsed().as_millis() as u64;
    let fast_url = fast.current_url().await?;
    let fast_verdict = format!("{fast_steps} decisions, model calls {escalated}, repairs {corrections}");
    println!("  → {:.2} s, {fast_steps} decisions, model calls {escalated}\n", fast_ms as f64 / 1000.0);
    push_event(&relay, "/push", json!({ "side": "fast", "finished": true, "total_ms": fast_ms,
        "url": fast_url, "verdict": fast_verdict }));

    println!("THE NUMBER FOR THE STAGE");
    println!("  ordinary agent: {:>7.1} s, {slow_steps} decisions", slow_ms as f64 / 1000.0);
    println!("  mint:         {:>7.2} s, {fast_steps} decisions, the model was asked {escalated} times", fast_ms as f64 / 1000.0);
    println!("  per decision: {:.0} ms against {:.3} ms — {:.0}x faster",
        slow_dec_us as f64 / slow_steps.max(1) as f64 / 1000.0,
        fast_dec_us as f64 / fast_steps.max(1) as f64 / 1000.0,
        (slow_dec_us as f64 / slow_steps.max(1) as f64) / (fast_dec_us as f64 / fast_steps.max(1) as f64).max(0.001));
    println!("  decisions for the same work: {slow_steps} against {fast_steps}");
    println!("  whole chain: {:.1} s against {:.2} s — {:.1}x", slow_ms as f64 / 1000.0, fast_ms as f64 / 1000.0,
        slow_ms as f64 / (fast_ms as f64).max(1.0));
    Ok(())
}

/// Re-observe and repair one step after its element went missing.
async fn cdp_reobserve(cdp: &mut Cdp, sk: &mut Skill, index: usize) -> Result<()> {
    // Re-observing moves the page again, so it is an action like any other and crosses the gate.
    let here_host = skill::host_of_url(&cdp.current_url().await.unwrap_or_default());
    let mut gate = gate::Gate::new(&here_host);
    cdp.settle(1500).await?;
    let (els, here) = cdp.state().await?;
    let step = sk.steps[index].clone();
    if let Some((idx, _c, _h)) = sk.resolve(&step, &els) {
        let el = els[idx].clone();
        let _ = perform(cdp, &mut gate, &step, index, &el, None).await?;
        sk.teach(index, &el, &els, &here, "");
    }
    Ok(())
}

/// Structured evaluation: run the compiled skills and report a verdict for every task.
///
/// The showcase counts decisions; an evaluation has to say whether the work was actually
/// done. A step that resolves but lands somewhere other than the page it promised is a
/// failure, and gets reported as one — a number of decisions with no verdict is not
/// evidence of anything.
async fn evaluate(args: &[String]) -> Result<()> {
    let jobs_path = flag(args, "--jobs").ok_or_else(|| anyhow!("--jobs is required"))?;
    let out_path = flag(args, "--out").unwrap_or_else(|| "evaluation.json".into());
    let labels_path = flag(args, "--labels");
    let category = flag(args, "--category").unwrap_or_else(|| "project tasks".into());
    // Where per-run receipts are written. A run that is signed but whose receipts are thrown away
    // is a run nobody can check, which is the same as not signing at all.
    let receipt_dir = flag(args, "--receipts");
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&jobs_path)?)?;

    // Targets are published as labels, not as addresses: the evaluation is about the
    // mechanism, and which repository it ran against is nobody's business.
    let labels: std::collections::HashMap<String, String> = match &labels_path {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?).unwrap_or_default(),
        None => Default::default(),
    };
    let label = |url: &str| labels.get(url).cloned().unwrap_or_else(|| url.to_string());

    if let Ok(closed) = Cdp::tidy_tabs(PORT).await {
        if closed > 0 {
            println!("  leftover tabs closed: {closed}");
        }
    }

    let mut cdp = Cdp::connect(PORT).await?;
    let mut runs: Vec<serde_json::Value> = Vec::new();
    let total = jobs.len();
    println!("evaluation: {total} tasks\n");

    for (i, job) in jobs.iter().enumerate() {
        let mut sk = Skill::load(&job.skill)?;
        let mut gate = gate::Gate::new(&sk.host);
        // Every run signs, so the evidence exists whether or not anyone asked for it in advance.
        sign_for(&mut gate, &sk.host);
        let goal = sk.goal.clone();
        cdp.navigate(&job.start).await?;
        cdp.settle(4000).await?;
        let started = Instant::now();
        let mut decisions = 0usize;
        let mut us: Vec<u64> = Vec::new();
        let mut tiers: Vec<String> = Vec::new();
        let mut escalations = 0usize;
        let mut corrections = 0usize;
        let mut log: Vec<String> = Vec::new();
        let mut broke = false;

        for idx in 0..sk.steps.len() {
            let (els, _here) = cdp.state().await?;
            let step = sk.steps[idx].clone();
            let t = Instant::now();
            let found = sk.resolve(&step, &els);
            let decision_us = t.elapsed().as_micros() as u64;
            let (el, tier) = match found {
                Some((k, _c, how)) => {
                    let name = match how {
                        Found::Shape => "shape",
                        Found::Words => "words",
                        Found::Taught => "taught",
                    };
                    (els[k].clone(), name.to_string())
                }
                None => {
                    escalations += 1;
                    // Same rule as in the chain: an unavailable model skips one job, it does not
                    // end the evaluation. A compiled path that cannot run without a model is not
                    // a compiled path.
                    match recover(&mut cdp, &sk.goal, &category, &mut log).await {
                        Ok(Some(el)) => (el, "model".to_string()),
                        Ok(None) => {
                            broke = true;
                            break;
                        }
                        Err(e) => {
                            println!("  the model is unavailable ({e}) — task skipped");
                            broke = true;
                            break;
                        }
                    }
                }
            };
            decisions += 1;
            us.push(decision_us);
            tiers.push(tier);
            let (done, blocked) = perform(&mut cdp, &mut gate, &step, idx, &el, None).await?;
            if let Some(why) = blocked {
                // The gate said no. The task stops here and says why; nothing is retried.
                log.push(format!("step {} not performed — {why}", idx + 1));
                broke = true;
                break;
            }
            if !done {
                cdp_reobserve(&mut cdp, &mut sk, idx).await?;
                corrections += 1;
                continue;
            }
            settle_until(&mut cdp, &step.produced_page, 2500).await?;
        }

        let wall_ms = started.elapsed().as_millis() as u64;
        let url = cdp.current_url().await?;
        let landed = state::page_kind(&url);
        let expected = sk
            .steps
            .last()
            .map(|s| s.produced_page.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| sk.expect_final_page.clone());
        let success = !broke && !expected.is_empty() && landed == expected;

        println!(
            "  {:>2}/{total} {:<22} {} decisions {}, model {}, repairs {}, {:.2} s",
            i + 1,
            label(&job.start),
            if success { "DONE    " } else { "FAILED  " },
            decisions,
            escalations,
            corrections,
            wall_ms as f64 / 1000.0
        );

        runs.push(json!({
            "skill": std::path::Path::new(&job.skill)
                .file_stem().and_then(|s| s.to_str()).unwrap_or(&job.skill),
            "goal": goal,
            "target": label(&job.start),
            "success": success,
            "decisions": decisions,
            "decision_us": us,
            "tiers": tiers,
            "model_calls": escalations,
            "corrections": corrections,
            "wall_ms": wall_ms,
            "landed": landed,
            "expected": expected,
            // What the gate decided about every step of this run, and why it stopped any of them.
            "gate": {
                "allowed": gate.tally().0,
                "held": gate.tally().1,
                "refused": gate.tally().2,
                "blocked": gate.blocked(),
            },
            "receipts": receipt_summary(&gate, receipt_dir.as_deref(), i),
        }));
    }

    let ok = runs.iter().filter(|r| r["success"].as_bool() == Some(true)).count();
    let no_model = runs.iter().filter(|r| r["model_calls"].as_u64() == Some(0)).count();
    let out = json!({
        "collected_at": now_stamp(),
        "runs": runs,
        "summary": {
            "tasks": total,
            "succeeded": ok,
            "failed": total - ok,
            "zero_model_calls": no_model,
        }
    });
    std::fs::write(&out_path, serde_json::to_string_pretty(&out)?)?;
    println!("\n  completed {ok} of {total}; with no model call at all: {no_model} of {total}");
    println!("  report: {out_path}");
    Ok(())
}

/// Discovery before the compile: take a class of targets, cover it with one compiled skill, and
/// verify every single one.
///
/// The list can come from a discovery service, which is the half of the loop that finds the
/// class: it maps a site and hands back the pages of a kind. With no endpoint configured the
/// list is read from a file and the run still does the part that matters — one skill, many
/// targets, a verdict for each.
async fn discover(args: &[String]) -> Result<()> {
    let urls_path = flag(args, "--urls").ok_or_else(|| anyhow!("--urls is required"))?;
    let skill_path = flag(args, "--skill").ok_or_else(|| anyhow!("--skill is required"))?;
    let out_path = flag(args, "--out").unwrap_or_else(|| "coverage.json".into());
    let labels_path = flag(args, "--labels");
    let endpoint = flag(args, "--discovery");
    let seed = flag(args, "--seed");
    let category = flag(args, "--category").unwrap_or_else(|| "project tasks".into());

    let mut targets: Vec<String> = serde_json::from_str(&std::fs::read_to_string(&urls_path)?)?;

    if let (Some(base), Some(seed_url)) = (&endpoint, &seed) {
        let key = std::env::var("DISCOVERY_API_KEY").unwrap_or_default();
        let resp = ureq::post(&format!("{}/v1/map", base.trim_end_matches('/')))
            .set("Authorization", &format!("Bearer {key}"))
            .set("Content-Type", "application/json")
            .send_json(json!({ "url": seed_url, "search": "" }));
        match resp {
            Ok(r) => {
                let v: serde_json::Value = r.into_json()?;
                let mut added = 0usize;
                if let Some(links) = v.get("links").and_then(|l| l.as_array()) {
                    for l in links.iter().filter_map(|l| l.as_str()) {
                        if !targets.iter().any(|t| t == l) {
                            targets.push(l.to_string());
                            added += 1;
                        }
                    }
                }
                println!("  discovery added {added} targets to the class");
            }
            Err(e) => println!("  the discovery endpoint did not answer ({e}) - using the list as given"),
        }
    }

    let labels: std::collections::HashMap<String, String> = match &labels_path {
        Some(p) => serde_json::from_str(&std::fs::read_to_string(p)?).unwrap_or_default(),
        None => Default::default(),
    };
    let label = |url: &str| labels.get(url).cloned().unwrap_or_else(|| url.to_string());

    let base_skill = Skill::load(&skill_path)?;
    if let Ok(closed) = Cdp::tidy_tabs(PORT).await {
        if closed > 0 {
            println!("  leftover tabs closed: {closed}");
        }
    }
    let mut cdp = Cdp::connect(PORT).await?;
    let total = targets.len();
    let mut covered = 0usize;
    let mut results: Vec<serde_json::Value> = Vec::new();
    println!("one skill, a class of {total} targets\n");

    for (i, target) in targets.iter().enumerate() {
        let mut sk = base_skill.clone();
        let mut gate = gate::Gate::new(&sk.host);
        cdp.navigate(target).await?;
        cdp.settle(4000).await?;
        let started = Instant::now();
        let mut decisions = 0usize;
        let mut model_calls = 0usize;
        let mut log: Vec<String> = Vec::new();
        let mut broke = false;

        for idx in 0..sk.steps.len() {
            let (els, _here) = cdp.state().await?;
            let step = sk.steps[idx].clone();
            let found = sk.resolve(&step, &els);
            let (el, _tier) = match found {
                Some((k, _c, _h)) => (els[k].clone(), "shape"),
                None => {
                    model_calls += 1;
                    match recover(&mut cdp, &sk.goal, &category, &mut log).await {
                        Ok(Some(el)) => (el, "model"),
                        Ok(None) => {
                            broke = true;
                            break;
                        }
                        Err(e) => {
                            println!("  the model is unavailable ({e}) - target skipped");
                            broke = true;
                            break;
                        }
                    }
                }
            };
            decisions += 1;
            let (done, blocked) = perform(&mut cdp, &mut gate, &step, idx, &el, None).await?;
            if let Some(why) = blocked {
                log.push(format!("step {} not performed — {why}", idx + 1));
                broke = true;
                break;
            }
            if !done {
                cdp_reobserve(&mut cdp, &mut sk, idx).await?;
                continue;
            }
            settle_until(&mut cdp, &step.produced_page, 2500).await?;
        }

        let wall_ms = started.elapsed().as_millis() as u64;
        let url = cdp.current_url().await?;
        let landed = state::page_kind(&url);
        let expected = sk
            .steps
            .last()
            .map(|s| s.produced_page.clone())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| sk.expect_final_page.clone());
        let success = !broke && !expected.is_empty() && landed == expected;
        if success {
            covered += 1;
        }
        println!(
            "  {:>3}/{total} {:<34} {}",
            i + 1,
            label(target),
            if success { "COVERED" } else { "MISSED" }
        );
        results.push(json!({
            "target": label(target),
            "covered": success,
            "decisions": decisions,
            "model_calls": model_calls,
            "wall_ms": wall_ms,
            "landed": landed,
            "expected": expected,
        }));
    }

    let out = json!({
        "collected_at": now_stamp(),
        "skill": std::path::Path::new(&skill_path).file_stem().and_then(|s| s.to_str()).unwrap_or(&skill_path),
        "goal": base_skill.goal,
        "targets": total,
        "covered": covered,
        "results": results,
    });
    std::fs::write(&out_path, serde_json::to_string_pretty(&out)?)?;
    println!("\n  covered {covered} of {total}; report: {out_path}");
    Ok(())
}

/// The trace store: every verified run written as documents a reactive backend could hold.
///
/// The shape is deliberate. One document per run, one per step, and every step carries the tier
/// that resolved it plus the page it produced. That is what a store of this kind keeps, so
/// putting it behind one is a write, not a redesign.
async fn traces(args: &[String]) -> Result<()> {
    let eval_path = flag(args, "--eval").unwrap_or_else(|| "demo/evaluation.json".into());
    let out_path = flag(args, "--out").unwrap_or_else(|| "demo/traces.jsonl".into());
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&eval_path)?)?;
    let runs = v.get("runs").and_then(|r| r.as_array()).cloned().unwrap_or_default();

    let mut buf = String::new();
    let mut n_runs = 0usize;
    let mut n_steps = 0usize;
    for r in &runs {
        n_runs += 1;
        let id = format!("run_{n_runs}");
        buf.push_str(&serde_json::to_string(&json!({
            "_table": "runs",
            "runId": id,
            "skill": r["skill"],
            "target": r["target"],
            "goal": r["goal"],
            "success": r["success"],
            "decisions": r["decisions"],
            "modelCalls": r["model_calls"],
            "wallMs": r["wall_ms"],
            "landed": r["landed"],
            "expected": r["expected"],
            "at": v["collected_at"],
        }))?);
        buf.push('\n');
        let tiers = r["tiers"].as_array().cloned().unwrap_or_default();
        let us = r["decision_us"].as_array().cloned().unwrap_or_default();
        for (i, tier) in tiers.iter().enumerate() {
            n_steps += 1;
            buf.push_str(&serde_json::to_string(&json!({
                "_table": "steps",
                "runId": id,
                "step": i + 1,
                "tier": tier,
                "decisionUs": us.get(i).cloned().unwrap_or(json!(0)),
                "verified": r["success"],
            }))?);
            buf.push('\n');
        }
    }
    std::fs::write(&out_path, buf)?;
    println!("  wrote {n_runs} runs and {n_steps} steps to {out_path}");
    println!("  every step carries the tier that resolved it - the field a live view reads");
    Ok(())
}

/// Shape a compiled skill as a marketplace listing.
///
/// The marketplace sells data plus a rule: entries of terms and answers, and prose on its own is
/// not callable. A compiled skill is a program rather than prose, but its steps already carry
/// both halves — the words an element is recognised by, and the outcome the step must produce —
/// so the listing is a projection of the skill, not a rewrite of it.
async fn arli_payload(args: &[String]) -> Result<()> {
    let skill_path = flag(args, "--skill").ok_or_else(|| anyhow!("--skill is required"))?;
    let out_path = flag(args, "--out").unwrap_or_else(|| "arli-payload.json".into());
    let price: f64 = flag(args, "--price").and_then(|v| v.parse().ok()).unwrap_or(0.20);
    let category = flag(args, "--category").unwrap_or_else(|| "automation".into());
    let subcategory = flag(args, "--subcategory").unwrap_or_else(|| "procedures".into());

    let sk = Skill::load(&skill_path)?;
    let size = std::fs::metadata(&skill_path).map(|m| m.len()).unwrap_or(0);

    let entries: Vec<serde_json::Value> = sk
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let terms: Vec<String> = [s.seen_label.clone(), s.name_shape.clone(), s.href_kind.clone()]
                .into_iter()
                .filter(|t| !t.trim().is_empty())
                .collect();
            let from = if s.learned_page.is_empty() { "the starting page" } else { s.learned_page.as_str() };
            let to = if s.produced_page.is_empty() { "the goal page" } else { s.produced_page.as_str() };
            json!({
                "title": format!("Step {}: {}", i + 1, if s.intent.is_empty() { "act" } else { s.intent.as_str() }),
                "terms": terms,
                "answer": format!("{from} -> {to}"),
            })
        })
        .collect();

    let payload = json!({
        "name": format!("{} (compiled)", sk.goal),
        "why": "An executable procedure compiled from a single demonstration. It runs without a \
                model, verifies every step against the page it must produce, and reports which \
                tier resolved each one.",
        "price_usd": price,
        "category": category,
        "subcategory": subcategory,
        "rights_declared": true,
        "entries": entries,
        "content": format!(
            "# {}\n\nCompiled by mint from one demonstration on {}.\n\n{} step(s), {size} bytes of \
             readable JSON. Each step states the shape it recognises and the page it must produce; \
             a run verifies both and reports the tier that answered: shape, words or model. \
             Marginal cost of an execution is zero model calls.\n",
            sk.goal,
            sk.learned_from,
            sk.steps.len()
        ),
        "harness": {
            "runtime": "mint",
            "skill_file": std::path::Path::new(&skill_path)
                .file_name().and_then(|s| s.to_str()).unwrap_or(&skill_path),
            "steps": sk.steps.len(),
            "bytes": size,
            "verified_goal": sk.learn_declared_done,
            "model_calls_to_learn": sk.learn_model_calls,
            "run": "mint arli-serve --skill <file> --slug <slug> --port <port>",
        },
    });
    std::fs::write(&out_path, serde_json::to_string_pretty(&payload)?)?;
    println!(
        "  listing written to {out_path}: {} entries, ${price:.2} per call",
        payload["entries"].as_array().map(|a| a.len()).unwrap_or(0)
    );

    // Submit for real when a dashboard token is present; otherwise the payload is the deliverable.
    if let Ok(token) = std::env::var("ARLI_DASHBOARD_TOKEN") {
        if !token.trim().is_empty() {
            let resp = ureq::post("https://arliwork.com/v1/skill/from-harness")
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .send_json(payload.clone());
            match resp {
                Ok(r) => {
                    let v: serde_json::Value = r.into_json().unwrap_or(json!({}));
                    println!("  submitted as a draft: {}", serde_json::to_string(&v)?);
                    println!("  publish with POST /v1/skill/publish when the draft looks right");
                }
                Err(e) => println!("  submission refused ({e}) - the payload is still valid"),
            }
        }
    } else {
        println!("  no dashboard token set, so nothing was submitted: the payload is ready to post");
    }
    Ok(())
}

fn now_stamp() -> String {
    std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M %Z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn trim(s: &str) -> String {
    s.chars().take(70).collect()
}
