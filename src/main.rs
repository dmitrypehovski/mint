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

use mint::{approval, checkpoint, decide, endorse, gate, policy, receipt, shell, skill, state};

use anyhow::{anyhow, Result};
use mint::cdp::{Cdp, Element};
use mint::skill::{Action, Found, Resolution, Skill, Step};
use serde_json::json;
use sha2::{Digest, Sha256};
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
        "dir-learn" => dir_learn(&args).await,
        "dir-run" => dir_run(&args).await,
        "dir-demo" => dir_demo(&args).await,
        "approve" => approve_cmd(&args),
        "revoke" => revoke_cmd(&args),
        "approve-check" => approve_check_cmd(&args),
        "checkpoint" => checkpoint_cmd(&args),
        "witness" => witness_cmd(&args),
        "verify-witness" => verify_witness_cmd(&args),
        "endorse" => endorse_cmd(&args),
        "rules" => rules_cmd(&args),
        "guard" => guard_cmd(&args),
        "hook" => hook_cmd(&args),
        "--version" | "-V" | "version" => {
            // A receipt is checked against a version of the rules and a version of this program, and an
            // install script has nothing else to print that tells a person what they just installed.
            println!("mint {} — {}", env!("CARGO_PKG_VERSION"), receipt::CLAIM);
            Ok(())
        }
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
    // MINT_DEBUG_STATE=2 prints exactly what the model is given, which is the only way
    // to tell a model that cannot see its own work from one that sees it and ignores it.
    if std::env::var("MINT_DEBUG_STATE").as_deref() == Ok("2") {
        println!("  [state as the model reads it]\n{}", state::as_table(&els));
    }
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
        Action::Click => cdp.click_handle(el).await?,
        Action::Type { text } => cdp.type_handle(el, text).await?,
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
/// Where a key lives when nothing says otherwise. Under the home directory rather than a fixed path, so
/// an install on somebody else's machine is not told to look in root's.
fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/root".into())
}

fn receipt_key() -> String {
    std::env::var("MINT_RECEIPT_KEY").unwrap_or_else(|_| format!("{}/.secrets/mint-receipt.key", home()))
}

/// The counterparty's key. Separate from the runtime's on purpose: an endorsement the runtime can
/// produce by itself is not a counter-signature, and this variable exists so that it cannot be one.
fn endorser_key() -> String {
    std::env::var("MINT_ENDORSER_KEY")
        .unwrap_or_else(|_| format!("{}/.secrets/mint-endorser.key", home()))
}

/// The key an approver signs with. Its own variable and its own file, for the same reason the
/// endorser's is separate: a runtime that can produce an approval has not been approved by anybody.
fn approver_key() -> String {
    std::env::var("MINT_APPROVER_KEY")
        .unwrap_or_else(|_| format!("{}/.secrets/mint-approver.key", home()))
}

/// The approvers this runtime will accept a signature from. Read where a run's gate is fitted out, so
/// every command gets the same arrangement instead of each remembering it.
fn pinned_approvers() -> approval::Approvers {
    let Ok(path) = std::env::var("MINT_APPROVERS") else {
        return approval::Approvers::empty();
    };
    match approval::Approvers::from_file(&path) {
        Ok(a) => {
            println!("  approvers pinned from {path}: {}", a.describe());
            a
        }
        Err(e) => {
            // Deliberately not an empty set: a pin that fails to load must refuse what it cannot check,
            // not quietly stop pinning.
            eprintln!("  the pinned approvers could not be read from {path}: {e}");
            eprintln!("  no approval will be accepted on this run");
            approval::Approvers::unreadable()
        }
    }
}

/// The approvals this run was handed, each of which still has to be checked against the action waiting.
fn presented_approvals() -> Vec<approval::Approval> {
    let Ok(paths) = std::env::var("MINT_APPROVAL_FILES") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in paths.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let text = std::fs::read_to_string(path);
        let parsed = text
            .ok()
            .and_then(|t| serde_json::from_str::<approval::Approval>(&t).ok());
        match parsed {
            Some(a) => {
                println!(
                    "  an approval is in hand: signed by {}, for «{}»",
                    &a.sig.kid[..12.min(a.sig.kid.len())],
                    a.body.action.chars().take(44).collect::<String>()
                );
                out.push(a);
            }
            None => eprintln!("  the approval at {path} could not be read"),
        }
    }
    out
}

/// Issue an approval: one action, one runtime, an expiry, signed by the person answering for it.
///
/// Run by whoever is answering, with their own key, and given to the runtime as a file. The gate checks
/// the issuer against the approvers it was told to trust, so an approval is only ever as strong as that
/// list — which is the point of pinning it.
fn approve_cmd(args: &[String]) -> Result<()> {
    let action = flag(args, "--action").ok_or_else(|| {
        anyhow!("--action is required: the identity of the action being authorised, as the gate prints it")
    })?;
    let for_agent = match flag(args, "--for") {
        Some(a) => a,
        None => {
            let agent = receipt::Sealer::public_of(&receipt_key())?;
            println!(
                "  for the runtime whose key is {}: {}",
                receipt_key(),
                &agent[..12.min(agent.len())]
            );
            agent
        }
    };
    let days: u128 = flag(args, "--days").and_then(|v| v.parse().ok()).unwrap_or(1);
    let note = flag(args, "--note").unwrap_or_default();
    let sealer = receipt::Sealer::load_or_create(&approver_key())?;
    // A demonstration has to be able to show an expiry without waiting for one, and a test has to be
    // able to place an approval in time. Everything else uses the clock.
    let issued = flag(args, "--issued-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(endorse::now_ms);
    let not_after = flag(args, "--expires-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(issued + days * 86_400_000);
    let a = approval::Approval::issue(&sealer, &action, &for_agent, issued, not_after, &note);
    let out = flag(args, "--out").unwrap_or_else(|| "approval.json".into());
    std::fs::write(&out, serde_json::to_string_pretty(&serde_json::to_value(&a)?)?)?;

    println!("  approved «{}»", action.chars().take(44).collect::<String>());
    println!(
        "  by {} ({}), valid {days} day(s)",
        &sealer.agent[..12.min(sealer.agent.len())],
        approver_key()
    );
    println!("  written to {out}");
    println!("  the runtime has to pin this key to accept it:");
    println!("    MINT_APPROVERS=<a file listing {}> MINT_APPROVAL_FILES={out}", &sealer.agent[..12.min(sealer.agent.len())]);
    Ok(())
}

/// The log's lock, so two decisions taken at the same moment do not overwrite each other.
///
/// The log is a document that has to be rewritten in full to append to it, so two writers that read before
/// either writes lose one decision. Losing a *hold* is the worst loss there is: the command still did not
/// run, and nothing records that anybody was asked. Found on Codex, which issues several commands at once
/// where Claude Code issued them one at a time.
///
/// A stale lock is broken rather than waited on forever, because a hook that hangs is a hook that gets
/// switched off, and a switched-off gate protects nothing.
struct LogLock {
    path: String,
}

impl LogLock {
    fn take(log_path: &str) -> Option<LogLock> {
        let path = format!("{log_path}.lock");
        for _ in 0..100 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    // Whoever holds it says who they are, so a lock left behind by a process that is gone
                    // can be recognised as gone. This matters because the gate exits with `process::exit`,
                    // which runs no destructors: without the pid, the first guard to write a log left a
                    // lock behind and every later decision in that project was refused.
                    use std::io::Write;
                    let _ = write!(f, "{}", std::process::id());
                    return Some(LogLock { path });
                }
                Err(_) => {
                    if holder_is_gone(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
        None
    }
}

/// Whether the lock was left behind by a process that no longer exists, or is old enough to be one.
///
/// A lock that is never released is worse than no lock: it refuses everything, forever, and the person
/// turns the gate off. So the holder is asked whether it is still there.
fn holder_is_gone(path: &str) -> bool {
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            if pid != std::process::id() {
                return !std::path::Path::new(&format!("/proc/{pid}")).exists();
            }
        }
    }
    if let Ok(meta) = std::fs::metadata(path) {
        if let Ok(age) = meta
            .modified()
            .and_then(|t| t.elapsed().map_err(|_| std::io::Error::other("clock")))
        {
            return age.as_secs() > 10;
        }
    }
    false
}

impl Drop for LogLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The commands an agent must not run for itself, if the command names one.
///
/// A text match, and worth being plain about what that means: it catches `mint approve`, `approve.sh`,
/// and the same command hidden behind an obvious prefix, and it does not catch an agent that writes a
/// script and runs the script. Nothing in a harness can. The line that actually holds is a key the agent
/// cannot read, on a machine it does not run on — which is why the approval records how it was issued
/// instead of pretending the difference does not exist.
pub fn person_command(command: &str) -> Option<&'static str> {
    let flat = command.replace(['\n', '\t'], " ");
    let words: Vec<&str> = flat.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let bare = w.trim_matches(|c: char| c == '"' || c == '\'' || c == ';' || c == '&' || c == '(');
        if bare.ends_with("mint") || bare.ends_with("mint.sh") {
            if let Some(next) = words.get(i + 1) {
                match *next {
                    "approve" => return Some("mint approve"),
                    "revoke" => return Some("mint revoke"),
                    "endorse" => return Some("mint endorse"),
                    _ => {}
                }
            }
        }
        if bare.ends_with("approve.sh") {
            return Some("mint approve");
        }
        if bare.ends_with("revoke.sh") {
            return Some("mint revoke");
        }
    }
    None
}

/// The name of an action, which is what an approval is bound to.
///
/// For a shell command the command itself is part of the name, and that is not a detail: the observation
/// says a command writes, and nothing else — so without the text, every write in the world shares one
/// name. An approval for a commit would authorise a push, a deletion, a deploy. Found by building the
/// quickstart page, which showed the approval for one command being accepted for another.
///
/// The browser and directory paths pass no command: their actions are named by the element they act on,
/// which is already in the observation.
fn action_identity(observed: &policy::Observed, command: Option<&str>) -> String {
    let mut basis = serde_json::to_string(observed).unwrap_or_default();
    if let Some(cmd) = command {
        basis.push('\u{0}');
        basis.push_str(cmd);
    }
    format!(
        "guard:{}",
        hex_of(&Sha256::digest(basis.as_bytes()))[..16].to_string()
    )
}

/// Read the withdrawals a run was given. A file, or a comma-separated list of them, because a runtime
/// may be told about several people's withdrawals and has no business deciding whose to read first.
fn load_revocations(args: &[String]) -> Vec<approval::Revocation> {
    let paths = flag(args, "--revocations").or_else(|| std::env::var("MINT_REVOCATIONS").ok());
    let Some(paths) = paths else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in paths.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let parsed = std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<approval::Revocation>(&t).ok());
        match parsed {
            Some(r) => out.push(r),
            None => eprintln!("  the withdrawal at {path} could not be read"),
        }
    }
    out
}

/// The hook Claude Code runs before a tool call.
///
/// A skill is documentation: it asks the agent to consult the gate, and an agent that does not read it
/// simply commits. That was measured rather than assumed — a live session with the plugin installed
/// committed without asking and the log stayed empty. Documentation is not a gate.
///
/// This is the same gate in a place the agent cannot walk around. The harness runs it before every shell
/// command and obeys the answer, so a command the rules hold is held whatever the model decided, and a
/// command nobody can judge is refused. The agent's cooperation stops being part of the mechanism.
///
/// The payload arrives on stdin in Claude Code's shape and the answer leaves on stdout in the same shape.
/// Nothing here reads the transcript, the model or the conversation — only the command.
fn hook_cmd(args: &[String]) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Payload {
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        tool_input: serde_json::Value,
        #[serde(default)]
        cwd: String,
    }

    fn say(kind: &str, reason: &str) -> serde_json::Value {
        serde_json::json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": kind,
                "permissionDecisionReason": reason,
            }
        })
    }

    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
    let payload: Payload = match serde_json::from_str(&text) {
        Ok(p) => p,
        Err(e) => {
            // A payload we cannot read is a call we cannot judge, and guessing in the permissive
            // direction is the one failure this function exists to avoid.
            println!(
                "{}",
                say(
                    "deny",
                    &format!("mint could not read the hook payload ({e}), so this command was not judged")
                )
            );
            return Ok(());
        }
    };

    let command = payload
        .tool_input
        .get("command")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if payload.tool_name != "Bash" || command.is_empty() {
        println!("{{}}");
        return Ok(());
    }

    // The one command an agent must not be able to run for itself. A gate whose keeper can lift it is a
    // gate in name only, and this is the surface we actually control: every shell command the agent runs
    // passes through here. The refusal is recorded like any other, because "the agent tried to approve
    // itself" is the line in the log somebody most wants to read.
    let person_act = crate::person_command(command);

    let rules = match flag(args, "--policy") {
        Some(p) => policy::Policy::from_file(&p)?,
        None => policy::Policy::shipped(),
    };
    let observed = shell::observed(command, false);
    let identity = action_identity(&observed, Some(command));

    let dir = flag(args, "--receipts")
        .or_else(|| std::env::var("MINT_RECEIPTS").ok())
        .unwrap_or_else(|| {
            let base = if payload.cwd.is_empty() { ".".to_string() } else { payload.cwd.clone() };
            format!("{base}/.mint")
        });
    let log_path = format!("{}/guard.json", dir.trim_end_matches('/'));
    // The directory first: the lock is a file inside it, and on the very first call in a project there is
    // no such file to create it beside. Getting this wrong made every command fail closed on a fresh
    // project — safe, and useless.
    if let Err(e) = std::fs::create_dir_all(dir.trim_end_matches('/')) {
        println!(
            "{}",
            say("deny", &format!("mint could not create {dir} for its log ({e}), so this command did not run"))
        );
        return Ok(());
    }
    // Held for the whole read-decide-write, because the write rewrites the document: two hooks running at
    // once would each append to what they read, and the second would erase the first.
    let lock = LogLock::take(&log_path);
    if lock.is_none() {
        println!(
            "{}",
            say(
                "deny",
                "mint could not take the log's lock, so this command was not judged and did not run. \
                 Something else is writing to the log; try again."
            )
        );
        return Ok(());
    }
    let carried = receipt::read_log(&log_path).unwrap_or_default();

    let mut said = policy::ruling(&observed, &rules);
    let mut decided_by = "policy";
    if let Some(what) = person_act {
        said = policy::Ruling {
            class: said.class,
            verdict: "refuse".into(),
            reason: format!(
                "`{what}` is a person's act and this session is not a person. An approval or a withdrawal \
                 has to come from outside the agent: ask the human to run it in their own terminal."
            ),
        };
        decided_by = "not-a-person";
    } else if let Some(prior) = receipt::released_before(&carried, &identity) {
        said = policy::Ruling {
            class: said.class,
            verdict: "refuse".into(),
            reason: format!(
                "this action was already released once, as receipt {}; a repeat is not performed",
                prior.body.id
            ),
        };
        decided_by = "once-only";
    }

    // The record is written before the harness is told anything, including for a hold — a hold is the
    // decision somebody has to act on later, so a hold that is not in the log did not happen.
    //
    // And if it cannot be written, the command does not run. This used to swallow the error, which is how
    // a decision to hold a command disappeared from the log while the command was still refused: the
    // refusal was real and the record was silent, which is the one combination this product exists to
    // prevent.
    let recorded = (|| -> Result<()> {
        let sealer = receipt::Sealer::load_or_create(&receipt_key())?;
        let mut log = receipt::ReceiptLog::resume("guard", &rules, sealer, carried)?;
        let action = receipt::ActionRef {
            identity: identity.clone(),
            key: identity.clone(),
            side_effect: said.class.clone(),
            element: format!("command «{}»", short(command, 60)),
            step: 0,
            observed: observed.clone(),
            params_hash: None,
        };
        let decision_ref = receipt::DecisionRef {
            verdict: said.verdict.clone(),
            reason: said.reason.clone(),
            decided_by: decided_by.to_string(),
        };
        log.append_decision(action, decision_ref, endorse::now_ms());
        std::fs::create_dir_all(dir.trim_end_matches('/'))?;
        std::fs::write(&log_path, serde_json::to_string_pretty(&log.document())?)?;
        Ok(())
    })();
    if let Err(e) = recorded {
        println!(
            "{}",
            say(
                "deny",
                &format!(
                    "mint could not record the decision for this command ({e}), and a decision that is not \
                     recorded did not happen — so the command did not run."
                )
            )
        );
        return Ok(());
    }

    let out = match said.verdict.as_str() {
        "allow" => {
            println!("{{}}");
            return Ok(());
        }
        "hold" => say(
            "deny",
            &format!(
                "mint held this command before it ran: {}\n\
                 Nothing was executed. A person approves this exact action with:\n\
                   mint approve --action {identity} --days 1 --note \"why\" --out approval.json\n\
                 and the next attempt proceeds. Do not look for another way to run it.",
                said.reason
            ),
        ),
        _ => say("deny", &format!("mint refused this command: {}", said.reason)),
    };
    println!("{out}");
    Ok(())
}

/// Withdraw an approval: one action, or every approval issued before now.
///
/// An expiry can be waited out, which is no help when the reason for taking an approval back is that
/// something changed. This is signed like everything else, and only a key the runtime pins is obeyed —
/// otherwise anybody who could write a file could take back anybody's approval.
fn revoke_cmd(args: &[String]) -> Result<()> {
    let action = flag(args, "--action").ok_or_else(|| {
        anyhow!("--action is required: the action to withdraw, or * for every approval issued before now")
    })?;
    let sealer = receipt::Sealer::load_or_create(&approver_key())?;
    let at = flag(args, "--at-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(endorse::now_ms);
    let note = flag(args, "--note").unwrap_or_default();
    let r = approval::Revocation::issue(&sealer, &action, at, &note);
    let out = flag(args, "--out").unwrap_or_else(|| "revocation.json".into());
    std::fs::write(&out, serde_json::to_string_pretty(&serde_json::to_value(&r)?)?)?;

    if action == "*" {
        println!("  withdrawn: every approval issued before {at}");
    } else {
        println!("  withdrawn: the approval for {action}");
    }
    println!("  by {} ({})", short_hex(&sealer.agent), approver_key());
    println!("  written to {out}");
    println!("  a runtime has to be given it before it obeys it:");
    println!("    mint guard ... --revocations {out}   (or MINT_REVOCATIONS={out})");
    Ok(())
}

/// Put one approval through the checks a run would apply, and say which one it failed.
///
/// This exists so the checks can be *demonstrated* rather than described: each case on the page is a
/// real signed approval, and each is put through the same function the gate calls, with the same words
/// in the refusal.
fn approve_check_cmd(args: &[String]) -> Result<()> {
    let path = flag(args, "--approval").ok_or_else(|| anyhow!("--approval is required"))?;
    let action = flag(args, "--action").ok_or_else(|| anyhow!("--action is required"))?;
    let a: approval::Approval = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let pinned = match flag(args, "--approvers") {
        Some(p) => approval::Approvers::from_file(&p)?,
        // Nothing pinned is not a free pass here either: without a key to check, a signed approval
        // cannot be accepted, because there is nobody the runtime was told to trust.
        None => approval::Approvers::unreadable(),
    };
    let agent = match flag(args, "--agent") {
        Some(a) => a,
        None => receipt::Sealer::public_of(&receipt_key()).unwrap_or_default(),
    };
    let now = flag(args, "--at")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(endorse::now_ms);

    println!("  the approval          {}", path);
    println!("  signed by             {}", short_hex(&a.sig.kid));
    println!("  it authorises         {}", a.body.action);
    println!("  issued for the runtime {}", short_hex(&a.body.for_agent));
    println!("  from {} until {}", a.body.issued_at_ms, a.body.not_after_ms);
    println!("  approvers this runtime pins: {}", pinned.describe());
    println!("  checked at {now} by the rules in force");

    match approval::check(&a, &pinned, &action, &agent, now) {
        approval::Check::Accepted { approver, name } => {
            println!(
                "  verdict: accepted — {} authorised this action, and nothing about the approval is out of place",
                if name.is_empty() { short_hex(&approver) } else { name }
            );
            Ok(())
        }
        approval::Check::Refused { why } => {
            println!("  verdict: refused — {why}");
            anyhow::bail!("the approval does not hold")
        }
    }
}

fn short_hex(s: &str) -> String {
    s.chars().take(12).collect()
}

fn sign_for(gate: &mut gate::Gate, host: &str) {
    let rules = gate.policy().clone();
    match receipt::Sealer::load_or_create(&receipt_key()) {
        Ok(sealer) => gate.sign_with(receipt::ReceiptLog::new(host, &rules, sealer)),
        // A run must never fail because signing is unavailable — but it must never claim to be
        // signed when it is not, so the absence is said out loud.
        Err(e) => eprintln!("  receipts are off for this run: {e}"),
    }
    // Fitted out in the same place for every command: who may approve, and what they approved.
    gate.trust_approvers(pinned_approvers());
    gate.present_approvals(presented_approvals());
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
            let replay = if r.replayed + r.accepted_though_state_dependent > 0 {
                // Counted apart, because they are different claims. A receipt the rules reproduce is one
                // anybody can re-derive; a receipt the runtime decided from what had already happened, or
                // from whose act a command is, cannot be re-derived by anybody — it was accepted because
                // it refuses at least as much as the rules require, which is weaker.
                let mut s = format!(", and {} of them reproduced from the published rules", r.replayed);
                if r.accepted_though_state_dependent > 0 {
                    s.push_str(&format!(
                        ", plus {} the rules cannot reproduce — decided from what this runtime had already \
                         done, or from whose act a command is — accepted because each refuses at least as \
                         much as the rules require",
                        r.accepted_though_state_dependent
                    ));
                }
                s
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
/// Judge one action, write the receipt, and say in the exit status what the answer was.
///
/// `mint rules` applies the document to a list of observations and prints what it says. That is the
/// right tool for reading a log back and the wrong one for an agent that is about to act: it writes
/// nothing down, so the action it just permitted leaves no trace anywhere.
///
/// This is the same ruling for one action, with the two things a caller needs. The receipt, so the
/// decision can be checked afterwards by somebody who trusts neither the agent nor its operator. And a
/// status a script can branch on: **0 allowed, 2 held, 1 refused** — three rather than two, because
/// "wait for a person" and "no" are different answers, and a caller that cannot tell them apart will
/// treat a hold as a refusal and give up on work that was only waiting.
///
/// The identity of the action is a hash of the observation, so the same action asked about twice gets
/// the same name. That is what makes the loop work: a hold prints the name, a person approves that name
/// with `mint approve`, and the second call presents the approval and proceeds — with the approver's
/// name in the receipt.
fn guard_cmd(args: &[String]) -> Result<()> {
    let rules = match flag(args, "--policy") {
        Some(p) => policy::Policy::from_file(&p)?,
        None => policy::Policy::shipped(),
    };
    // Two ways in. `--command` is what an agent has in hand: the command it is about to run, read by the
    // same adapter that reads commands during a run, so a `git commit` is judged the way it would be
    // judged inside one. `--action` takes an observation already in the recorded shape, for callers that
    // read the state themselves.
    let (mut observed, described) = match flag(args, "--command") {
        Some(cmd) => (shell::observed(&cmd, false), format!("command «{}»", short(&cmd, 60))),
        None => {
            let source = flag(args, "--action").ok_or_else(|| {
                anyhow!("--action or --command is required: --action takes a file with one observation (or - for stdin)")
            })?;
            let text = if source == "-" {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                buf
            } else {
                std::fs::read_to_string(&source)?
            };
            let o: policy::Observed = serde_json::from_str(&text).map_err(|e| {
                anyhow!("{source} is not one observation in the recorded shape: {e}")
            })?;
            let described = format!("{} «{}»", o.action, o.tag);
            (o, described)
        }
    };

    // The name of this action, stable across calls, and specific to the command that was given.
    let identity = action_identity(&observed, flag(args, "--command").as_deref());

    // An approval, if one was presented. Checked here rather than trusted: the five checks a run applies
    // are the same five, against this exact action and this runtime.
    let mut approved_by: Option<String> = None;
    let mut approval_note = String::new();
    if let Some(path) = flag(args, "--approval") {
        let a: approval::Approval = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let pinned = match flag(args, "--approvers") {
            Some(p) => approval::Approvers::from_file(&p)?,
            None => pinned_approvers(),
        };
        let agent = receipt::Sealer::public_of(&receipt_key()).unwrap_or_default();
        let now = endorse::now_ms();
        match approval::check(&a, &pinned, &identity, &agent, now) {
            approval::Check::Accepted { name, .. } => {
                if a.body.issued_from.as_deref() == Some("not-a-terminal") {
                    println!(
                        "  note: this approval was not issued from a terminal — a script, a job, or an agent \
                         ran the command. It is recorded as such in the receipt."
                    );
                }
                // Checked after the approval holds, not instead of it: a withdrawal reaches an approval
                // that was already given, and says so in the words of whoever withdrew it.
                let revocations = load_revocations(args);
                match approval::revoked(&revocations, &pinned, &identity, a.body.issued_at_ms, now) {
                    Some(why) => {
                        println!("  the approval presented is withdrawn: {why}");
                        println!("  the action is judged as it was presented, without it");
                    }
                    None => {
                        observed.approval = true;
                        approval_note = format!("\n  approved by {name}, {path}");
                        approved_by = Some(name);
                    }
                }
            }
            approval::Check::Refused { why } => {
                println!("  the approval presented does not hold: {why}");
                println!("  the action is judged as it was presented, without it");
            }
        }
    }

    // The log, read before the ruling rather than after it: an approval is spent when the action it
    // names has been performed, and that has to hold across restarts, days and machines. Memory cannot
    // answer this question — a process that has just started has no memory, and one that never stopped
    // cannot see the machine that ran yesterday.
    let log_path = flag(args, "--receipts").map(|d| format!("{}/guard.json", d.trim_end_matches('/')));
    // Held across the read and the write, for the same reason the hook holds it: appending rewrites the
    // whole document, so two guards finishing at once would lose one of the two decisions.
    if let Some(p) = &log_path {
        if let Some(dir) = p.rsplit_once('/').map(|(d, _)| d) {
            std::fs::create_dir_all(dir)?;
        }
    }
    let _lock = log_path.as_ref().and_then(|p| LogLock::take(p));
    if log_path.is_some() && _lock.is_none() {
        anyhow::bail!("could not take the log's lock; something else is writing to it");
    }
    let carried = match &log_path {
        Some(p) => receipt::read_log(p)?,
        None => Vec::new(),
    };

    let mut said = policy::ruling(&observed, &rules);
    // Whose answer this is. A refusal from the rules can be reached again by anyone holding the document;
    // a refusal because this action was already performed cannot — it depends on what happened before,
    // which no rule can see. Saying which one it was is what keeps the log verifiable: without it, a
    // verifier applying the document reaches "allow" and finds a receipt that says otherwise.
    let mut decided_by = "policy";
    if let Some(prior) = receipt::released_before(&carried, &identity) {
        said = policy::Ruling {
            class: said.class,
            verdict: "refuse".into(),
            reason: format!(
                "this action was already released once, as receipt {}; a repeat is not performed",
                prior.body.id
            ),
        };
        decided_by = "once-only";
    }
    println!("  {:<7} {:<6} — {}", said.class, said.verdict, said.reason);
    println!(
        "  decided by {} v{}, rules {}",
        policy::SPEC,
        rules.version,
        &rules.hash()[..16]
    );
    println!("  the action is called {identity}{approval_note}");

    // The receipt. Written before the answer is returned, so an agent that crashes between deciding and
    // acting still leaves the decision behind.
    if let Some(path) = &log_path {
        std::fs::create_dir_all(path.rsplit_once('/').map(|(d, _)| d).unwrap_or(".")).ok();
        let sealer = receipt::Sealer::load_or_create(&receipt_key())?;
        let mut log = {
            let log = receipt::ReceiptLog::resume("guard", &rules, sealer, carried.clone())?;
            log.approved_by(approved_by.clone())
        };
        let action = receipt::ActionRef {
            identity: identity.clone(),
            key: identity.clone(),
            side_effect: said.class.clone(),
            element: described.clone(),
            step: 0,
            observed: observed.clone(),
            params_hash: None,
        };
        let decision = receipt::DecisionRef {
            verdict: said.verdict.clone(),
            reason: said.reason.clone(),
            decided_by: decided_by.to_string(),
        };
        let r = log.append_decision(action, decision, endorse::now_ms());
        std::fs::write(path, serde_json::to_string_pretty(&log.document())?)?;
        println!(
            "  receipt {} written to {path} — the log now holds {}",
            r.body.id,
            log.len()
        );
    } else {
        println!("  no receipt was written: give --receipts <dir> to keep the record");
    }

    // The status is what a caller branches on, and a piped stdout is block-buffered: without this the
    // verdict can be lost on the way out, which would leave an agent with a status and no reason.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    match said.verdict.as_str() {
        "allow" => Ok(()),
        "hold" => std::process::exit(2),
        _ => std::process::exit(1),
    }
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

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
/// Sign the head of a log.
///
/// A chain shows that nothing inside it was edited. It cannot show that the chain is the whole chain,
/// because whoever holds it can drop the last receipts and present a shorter chain that verifies
/// perfectly. This is the object that a second party can put its own signature on, so that a log which
/// later shrinks is caught by somebody other than its author.
fn checkpoint_cmd(args: &[String]) -> Result<()> {
    let doc_path = flag(args, "--receipts").ok_or_else(|| anyhow!("--receipts is required"))?;
    let text = std::fs::read_to_string(&doc_path)?;
    let doc: serde_json::Value = serde_json::from_str(&text)?;
    let head = checkpoint::head_of(&doc)?;

    // The checkpoint must be signed by the key the log itself names. A head signed by some other key
    // is a statement about somebody else's log, and a witness that countersigned it would be putting
    // its name on the wrong document.
    let named = doc
        .get("receipts")
        .and_then(serde_json::Value::as_array)
        .and_then(|r| r.first())
        .and_then(|r| r.get("body"))
        .and_then(|b| b.get("agent"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sealer = receipt::Sealer::load(&receipt_key())?;
    if !named.is_empty() && named != sealer.agent {
        anyhow::bail!(
            "the log names the agent {} and this key is {}: a checkpoint has to be signed by the key \
             whose log it is",
            &named[..12.min(named.len())],
            &sealer.agent[..12.min(sealer.agent.len())]
        );
    }

    let cp = checkpoint::sign(&sealer, head.clone(), endorse::now_ms());
    let out = flag(args, "--out").unwrap_or_else(|| "checkpoint.json".into());
    std::fs::write(&out, serde_json::to_string_pretty(&serde_json::to_value(&cp)?)?)?;

    println!("  {} receipts, head at sequence {}", head.count, head.head_seq);
    println!("  head hash {}", &head.head_hash[..16.min(head.head_hash.len())]);
    println!("  signed by {} as {}", &sealer.agent[..12.min(sealer.agent.len())], out);
    println!("  document digest {}", &checkpoint::digest_of(&text)[..23]);
    Ok(())
}

/// Countersign a checkpoint. This is the witness's command, and it is meant to be run by somebody who
/// does not hold the log: only then is a second signature worth anything.
fn witness_cmd(args: &[String]) -> Result<()> {
    let cp_path = flag(args, "--checkpoint").ok_or_else(|| anyhow!("--checkpoint is required"))?;
    let cp: checkpoint::Checkpoint = serde_json::from_str(&std::fs::read_to_string(&cp_path)?)?;
    // Verified before it is signed. Countersigning an unverified claim turns the witness's key into a
    // stamp that anyone can obtain by asking.
    cp.verify()
        .map_err(|e| anyhow!("nothing was countersigned: {e}"))?;

    let ledger_path = flag(args, "--ledger").unwrap_or_else(|| "witness-ledger.json".into());
    let mut ledger: Vec<checkpoint::Head> = std::fs::read_to_string(&ledger_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let previously = ledger.last().cloned();

    let witness = receipt::Sealer::load_or_create(&endorser_key())?;
    let at_ms = endorse::now_ms();
    let cs = checkpoint::countersign(&witness, &cp, previously.clone(), at_ms)?;

    let out = flag(args, "--out").unwrap_or_else(|| "witness.json".into());
    std::fs::write(&out, serde_json::to_string_pretty(&serde_json::to_value(&cs)?)?)?;
    // The witness's own record of what it saw. Written only after a countersignature exists, so the
    // ledger never claims an observation that was refused.
    ledger.push(cp.body.head.clone());
    std::fs::write(&ledger_path, serde_json::to_string_pretty(&ledger)?)?;

    println!(
        "  countersigned {} receipts at sequence {} for runtime {}",
        cp.body.head.count,
        cp.body.head.head_seq,
        &cp.body.runtime[..12.min(cp.body.runtime.len())]
    );
    println!("  witness key {}", &witness.agent[..12.min(witness.agent.len())]);
    match &previously {
        Some(prev) => println!(
            "  seen before: {} receipts at sequence {} — the log grew, which is allowed",
            prev.count, prev.head_seq
        ),
        None => println!("  first sighting of this log"),
    }
    println!("  written to {out}, and this sighting kept in {ledger_path}");
    Ok(())
}

/// Check a log against its checkpoint and the countersignature over it. Anybody can run this: it needs
/// no service, no network and no trust in either party.
fn verify_witness_cmd(args: &[String]) -> Result<()> {
    let doc_path = flag(args, "--receipts").ok_or_else(|| anyhow!("--receipts is required"))?;
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&doc_path)?)?;
    let cp_path = flag(args, "--checkpoint").ok_or_else(|| anyhow!("--checkpoint is required"))?;
    let cp: checkpoint::Checkpoint = serde_json::from_str(&std::fs::read_to_string(&cp_path)?)?;
    let w_path = flag(args, "--witness").ok_or_else(|| anyhow!("--witness is required"))?;
    let cs: checkpoint::Countersignature = serde_json::from_str(&std::fs::read_to_string(&w_path)?)?;

    let log_head = checkpoint::head_of(&doc).ok();
    let report = checkpoint::check(log_head.as_ref(), &cp, &cs);

    println!("  checkpoint says: {} receipts, sequence {}", cp.body.head.count, cp.body.head.head_seq);
    println!("  the log holds:   {} receipts", log_head.as_ref().map(|h| h.count).unwrap_or(0));
    println!(
        "  countersigned by: {} (the runtime is {})",
        &cs.body.witness[..12.min(cs.body.witness.len())],
        &cs.body.runtime[..12.min(cs.body.runtime.len())]
    );
    if report.ok {
        println!("\n  the log presented is the log that was countersigned, and it has not shrunk");
        println!("  the chain inside it is checked separately: mint receipts --verify --receipts {doc_path}");
        Ok(())
    } else {
        println!("\n  the check did not hold:");
        for f in &report.findings {
            println!("   - {f}");
        }
        anyhow::bail!("{} finding(s)", report.findings.len())
    }
}

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
// ── the directory surface ────────────────────────────────────────────────────────────────────────
//
// The same runtime pointed at a directory instead of a page. The browser came first because it is the
// hardest surface — the page redraws under you, the contents change, nothing is versioned. Nothing in
// the middle knows the difference: the decision, the gate, the rules and the receipts are the same
// objects, and a document written for a page governs a folder, because it never knew what a page was.

/// Learn a task that is a sequence of commands in one directory.
async fn dir_learn(args: &[String]) -> Result<()> {
    let dir = flag(args, "--dir").ok_or_else(|| anyhow!("--dir is required"))?;
    let goal = flag(args, "--goal").ok_or_else(|| anyhow!("--goal is required"))?;
    let out = flag(args, "--skill").unwrap_or_else(|| "dir.skill.json".into());
    let max: usize = flag(args, "--max-steps").and_then(|v| v.parse().ok()).unwrap_or(20);
    let rules = match flag(args, "--policy") {
        Some(p) => Some(policy::Policy::from_file(&p)?),
        None => None,
    };
    let sh = shell::Shell::new(&dir)?;
    let host = format!("dir:{dir}");
    let mut gate = gate::Gate::new(&host);
    if let Some(r) = rules {
        gate.adopt_policy(r);
    }
    sign_for(&mut gate, &host);

    println!("FIRST RUN IN A DIRECTORY — the model decides every command\n");
    println!("directory: {dir}\ngoal: {goal}");
    match flag(args, "--policy") {
        Some(p) => println!("learned under the rules in {p}\n"),
        None => println!("learned under the shipped rules: anything that changes files is held\n"),
    }

    let started = Instant::now();
    let mut calls = 0u32;
    let mut misses = Misses::default();
    let mut trail: Vec<String> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut declared_done = false;
    let mut expect_final = sh.kind()?;

    for _round in 0..max {
        if started.elapsed().as_secs() > LEARN_BUDGET_SECS {
            println!(
                "  the run has taken {} minutes, the declared budget — stopping",
                LEARN_BUDGET_SECS / 60
            );
            break;
        }
        let entries = sh.entries()?;
        let here = sh.kind()?;
        let d = match decide::next_command(&goal, &dir, &entries, &trail) {
            Ok(d) => d,
            Err(e) => {
                println!("  the model's answer could not be acted on: {e}");
                calls += 1;
                // A reply naming something that is not on the page is a wrong answer: worth one more
                // try with the failure spelled out, but waiting will not fix it.
                if misses.note(Miss::Retryable) {
                    println!("  two answers in a row could not be acted on — stopping");
                    break;
                }
                continue;
            }
        };
        calls += 1;
        let Some(idx) = d.idx else {
            let (ok, why) = decide::judge_success(&goal, &dir, "directory", &sh.listing()?, &trail)?;
            declared_done = ok;
            println!("   model: goal reached ({})", d.intent);
            println!("  goal check: {} — {why}", if ok { "reached" } else { "NOT reached" });
            break;
        };
        let el = entries[idx].clone();
        let Some(cmd) = d.text.clone() else {
            println!("  no command came with that step — telling the model and going on");
            if misses.note(Miss::Retryable) {
                println!("  two steps in a row without a command — stopping");
                break;
            }
            continue;
        };
        misses.progress();

        let mut step = skill::step_from(&el, Action::Click, &d.intent, d.confidence);
        step.cmd = Some(cmd.clone());
        step.learned_page = here.clone();

        // The decision is written before anything runs, on this surface as much as on a page.
        let (verdict, key) = gate.decide(&step, steps.len(), &el, None);
        if !verdict.performed() {
            println!("  the gate did not allow this step: {} — {}", verdict.as_str(), verdict.reason());
            break;
        }
        let (ran, output) = sh.run(&cmd)?;
        let after = sh.kind()?;
        let moved = after != here;
        step.produced_page = after.clone();
        gate.record_outcome(
            &key,
            ran && moved,
            if ran && moved {
                "the directory changed"
            } else if ran {
                "the command ran and changed nothing"
            } else {
                "the command failed"
            },
        );

        if ran && moved {
            println!("  step {}: {} 「{}」 — {}", steps.len() + 1, el.role, el.name, cmd);
            println!("    moved — the directory changed: {}", first_line(&output, &cmd));
            expect_final = after.clone();
            trail.push(format!("{cmd} — {output}"));
            steps.push(step);
        } else if ran {
            println!("  step {}: {} 「{}」 — {cmd}", steps.len() + 1, el.role, el.name);
            println!("    nothing changed in the directory — not counted as a step");
            trail.push(format!("{cmd} — changed nothing"));
        } else {
            println!("  step {}: {} 「{}」 — {cmd}", steps.len() + 1, el.role, el.name);
            println!("    the command failed: {output}");
            trail.push(format!("{cmd} — failed: {output}"));
        }
    }

    let sk = Skill {
        goal: goal.clone(),
        learned_from: host.clone(),
        host: host.clone(),
        expect_final_page: expect_final,
        steps,
        learn_model_calls: calls,
        learn_ms: started.elapsed().as_millis() as u64,
        learn_declared_done: declared_done,
    };
    sk.save(&out)?;
    println!("\n  result: {} steps, {calls} model calls, {:.1} s", sk.steps.len(), sk.learn_ms as f64 / 1000.0);
    if !declared_done {
        println!("  WARNING: this skill is marked unverified — do not present its success as proven");
    }
    println!("  the experience is written down as a program: {out}");
    receipt_summary(&gate, flag(args, "--receipts").as_deref(), 0);
    Ok(())
}

/// The same comparison on the directory surface: the same task, twice, on two fresh copies of the
/// same starting directory.
///
/// Both halves run under the same rules document — a comparison where one side is allowed less than
/// the other compares permissions rather than agents — and only the compiled half leaves a signed
/// log, because a signed record of every step is the thing the other kind of agent does not produce.
async fn dir_demo(args: &[String]) -> Result<()> {
    let path = flag(args, "--skill").ok_or_else(|| anyhow!("--skill is required"))?;
    let template = flag(args, "--dir").ok_or_else(|| anyhow!("--dir is required"))?;
    let relay = flag(args, "--push").unwrap_or_else(|| "http://127.0.0.1:8840".into());
    let max: usize = flag(args, "--max-steps").and_then(|v| v.parse().ok()).unwrap_or(70);
    let root = flag(args, "--work").unwrap_or_else(|| "/tmp/mint-dir-demo".into());
    let rules = match flag(args, "--policy") {
        Some(p) => Some(policy::Policy::from_file(&p)?),
        None => None,
    };
    let sk = Skill::load(&path)?;
    let goal = sk.goal.clone();

    // A fresh copy for each half: a task that moves files cannot be run twice in the same directory,
    // and comparing one side on work the other side has already done would be nonsense.
    let work_a = format!("{root}/model");
    let work_b = format!("{root}/skill");
    copy_tree(&template, &work_a)?;
    copy_tree(&template, &work_b)?;

    println!("showcase: {relay} | directory surface | goal: {goal}");
    println!("the same task, twice: a model deciding every command, and the compiled skill\n");

    let started = Instant::now();

    // ── side 1: the model decides every command ──────────────────────────────────────────────────
    let sh_a = shell::Shell::new(&work_a)?;
    let host_a = format!("dir:{work_a}");
    let mut gate_a = gate::Gate::new(&host_a);
    if let Some(r) = &rules {
        gate_a.adopt_policy(r.clone());
    }
    sign_for(&mut gate_a, &host_a);
    let mut trail: Vec<String> = Vec::new();
    let mut n = 0usize;
    let mut unusable = 0u32;
    let t1 = Instant::now();
    println!("ORDINARY AGENT — a model decides every command");
    for _ in 0..max {
        let entries = sh_a.entries()?;
        let here = sh_a.kind()?;
        let d = match decide::next_command(&goal, &work_a, &entries, &trail) {
            Ok(d) => d,
            Err(e) => {
                println!("  the model's answer could not be acted on: {e}");
                unusable += 1;
                if unusable >= 5 {
                    println!("  five answers that could not be acted on — stopping");
                    break;
                }
                continue;
            }
        };
        let Some(idx) = d.idx else {
            println!("   model: it considers the task finished");
            break;
        };
        let el = entries[idx].clone();
        let Some(cmd) = d.text.clone() else { continue };
        let (ran, output) = sh_a.run(&cmd)?;
        let after = sh_a.kind()?;
        let moved = after != here;
        n += 1;
        println!(
            "  step {n}: {} — {cmd} — {}",
            el.name,
            if ran && moved { "moved" } else { "no change" }
        );
        push_event(
            &relay,
            "/push",
            json!({ "side": "dir_model", "goal": goal, "asked": 1, "step": {
                "tier": "model", "n": n, "name": el.name, "cmd": cmd,
                "moved": ran && moved, "decision_us": 0,
                "output": output.chars().take(90).collect::<String>() } }),
        );
        trail.push(format!("{cmd} — {output}"));
        let _ = ran;
    }
    let slow_ms = t1.elapsed().as_millis() as u64;
    let (slow_ok, slow_why) = decide::judge_success(&goal, &work_a, "a directory", &sh_a.listing()?, &trail)?;
    println!(
        "\n  {} — {} commands, {:.1} s, {}",
        if slow_ok { "reached the goal" } else { "did NOT reach the goal" },
        n,
        slow_ms as f64 / 1000.0,
        slow_why
    );
    push_event(
        &relay,
        "/push",
        json!({ "side": "dir_model", "finished": true, "total_ms": slow_ms,
                "steps": n, "verdict": slow_why, "reached": slow_ok,
                "listing": sh_a.listing()? }),
    );

    // ── side 2: the compiled skill, no model at all ──────────────────────────────────────────────
    let sh_b = shell::Shell::new(&work_b)?;
    let host_b = format!("dir:{work_b}");
    let mut gate = gate::Gate::new(&host_b);
    if let Some(r) = &rules {
        gate.adopt_policy(r.clone());
    }
    sign_for(&mut gate, &host_b);
    let t2 = Instant::now();
    let mut done = 0usize;
    println!("\nMINT — the same task, compiled");
    for (i, step) in sk.steps.iter().enumerate() {
        let entries = sh_b.entries()?;
        let here = sh_b.kind()?;
        let found = match entries.iter().find(|e| e.name == step.seen_label) {
            Some(e) => Some(e.clone()),
            None => entries
                .iter()
                .find(|e| {
                    let (_, shape) = skill::describe(e);
                    shape == step.name_shape && e.role == step.role
                })
                .cloned(),
        };
        let Some(el) = found else { break };
        let Some(cmd) = step.cmd.clone() else { break };
        let (verdict, key) = gate.decide(step, i, &el, None);
        if !verdict.performed() {
            println!("  step {}: the gate held it — {}", i + 1, verdict.reason());
            break;
        }
        let d0 = Instant::now();
        let (ran, output) = sh_b.run(&cmd)?;
        let after = sh_b.kind()?;
        let moved = after != here;
        let decision_us = d0.elapsed().as_micros();
        gate.record_outcome(&key, ran && moved, if ran && moved { "the directory changed" } else { "no change" });
        println!(
            "  step {}: {} {} — {}",
            i + 1,
            el.name,
            if moved { "moved" } else { "no change" },
            cmd
        );
        push_event(
            &relay,
            "/push",
            json!({ "side": "dir_skill", "goal": goal, "step": {
                "tier": "skill", "n": i + 1, "name": el.name, "cmd": cmd,
                "moved": ran && moved, "decision_us": decision_us,
                "output": output.chars().take(90).collect::<String>() } }),
        );
        if ran && moved {
            done += 1;
        }
    }
    let fast_ms = t2.elapsed().as_millis() as u64;
    let (fast_ok, fast_why) = decide::judge_success(&goal, &work_b, "a directory", &sh_b.listing()?, &[])?;
    println!(
        "\n  {} — {} commands, {:.2} s, 0 model calls",
        if fast_ok { "reached the goal" } else { "did NOT reach the goal" },
        done,
        fast_ms as f64 / 1000.0
    );

    // The signed log, which is the whole difference between the two halves afterwards.
    let record = receipt_summary(&gate, flag(args, "--receipts").as_deref(), 1);
    push_event(
        &relay,
        "/push",
        json!({ "side": "dir_skill", "finished": true, "total_ms": fast_ms,
                "steps": done, "verdict": fast_why, "reached": fast_ok,
                "receipts": record, "listing": sh_b.listing()? }),
    );

    println!("\nTHE NUMBER FOR THE STAGE");
    println!(
        "  ordinary agent: {:.1} s, {n} commands, every one a question to a model",
        slow_ms as f64 / 1000.0
    );
    println!("  mint:          {:.2} s, {done} commands, model calls 0", fast_ms as f64 / 1000.0);
    println!("  the same directory, the same task, and the difference is not the clock: the");
    println!("  ordinary agent pays {n} questions to a model EVERY time this task is run. mint paid");
    println!("  them once, when the task was learned, and pays 0 now.");
    println!("  total on the stage: {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}

/// A fresh copy of a directory, one level deep, which is all these fixtures need.
fn copy_tree(from: &str, to: &str) -> Result<()> {
    let from = std::path::Path::new(from);
    let to = std::path::Path::new(to);
    if to.exists() {
        std::fs::remove_dir_all(to)?;
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Replay a learned directory task: the recorded commands, no model at all.
async fn dir_run(args: &[String]) -> Result<()> {
    let path = flag(args, "--skill").ok_or_else(|| anyhow!("--skill is required"))?;
    let dir = flag(args, "--dir").ok_or_else(|| anyhow!("--dir is required"))?;
    let rules = match flag(args, "--policy") {
        Some(p) => Some(policy::Policy::from_file(&p)?),
        None => None,
    };
    let sh = shell::Shell::new(&dir)?;
    let host = format!("dir:{dir}");
    let mut gate = gate::Gate::new(&host);
    if let Some(r) = rules {
        gate.adopt_policy(r);
    }
    sign_for(&mut gate, &host);
    let sk = Skill::load(&path)?;

    println!("REPLAY — the recorded commands, and no model\n");
    println!("learned in: {}", sk.learned_from);
    println!("what it cost back then: {} model calls, {:.1} s", sk.learn_model_calls, sk.learn_ms as f64 / 1000.0);
    println!("the goal check confirmed success: {}\n", if sk.learn_declared_done { "yes" } else { "NO — the skill is unverified" });

    let started = Instant::now();
    let mut done = 0usize;
    for (i, step) in sk.steps.iter().enumerate() {
        let entries = sh.entries()?;
        let here = sh.kind()?;
        // The entry the step means, found by its shape — the same reading as on a page, and the reason
        // a skill learned in one directory works in another one whose files are named differently.
        let found = match entries.iter().find(|e| e.name == step.seen_label) {
            Some(e) => Some(e.clone()),
            None => entries
                .iter()
                .find(|e| {
                    // Nothing with that exact name: match by the shape of the name instead, which is
                    // how a skill learned in one directory fits another whose files are named
                    // differently — the same reading as a page whose data has changed.
                    let (_, shape) = skill::describe(e);
                    shape == step.name_shape && e.role == step.role
                })
                .cloned(),
        };
        let Some(el) = found else {
            println!("  step {}: nothing in the directory matches — the skill does not fit here", i + 1);
            break;
        };
        let Some(cmd) = step.cmd.clone() else {
            println!("  step {}: no command recorded — this is not a directory skill", i + 1);
            break;
        };
        let (verdict, key) = gate.decide(step, i, &el, None);
        if !verdict.performed() {
            println!("  step {}: the gate held it — {}: {}", i + 1, verdict.as_str(), verdict.reason());
            println!("    this is where a skill that changes files stops to be approved, which is the");
            println!("    point of having a gate; nothing was run.");
            break;
        }
        let t = Instant::now();
        let (ran, output) = sh.run(&cmd)?;
        let after = sh.kind()?;
        let moved = after != here;
        gate.record_outcome(&key, ran && moved, if ran && moved { "the directory changed" } else { "no change" });
        println!(
            "  step {}: {} — {}  ({} µs of decision)",
            i + 1,
            cmd,
            if ran && moved { "moved" } else { "no change" },
            t.elapsed().as_micros()
        );
        if !output.is_empty() && !ran {
            println!("    {output}");
        }
        if ran && moved {
            done += 1;
        }
    }
    println!("\n  {} steps replayed, 0 model calls, {:.2} s", done, started.elapsed().as_secs_f64());
    println!("  landed on: {}", sh.kind()?);
    receipt_summary(&gate, flag(args, "--receipts").as_deref(), 2);
    Ok(())
}

fn first_line(output: &str, fallback: &str) -> String {
    let l = output.lines().next().unwrap_or("").trim();
    if l.is_empty() { fallback.to_string() } else { l.chars().take(60).collect() }
}

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
    // A verifier that prints a break and returns success is useless in a script: the exit code is the
    // only part a script reads. The relay that shows these demonstrations tolerates a non-zero code as
    // long as there is output to show, and a shell is not so forgiving.
    if !real.ok {
        anyhow::bail!("the log does not hold");
    }
    Ok(())
}

/// Why a step did not happen, and what the run should do about it.
///
/// The old rule counted every failure the same way and stopped after two in a row, which is wrong in
/// both directions. An element that is in the page but out of reach may be reachable a moment later,
/// and a model that named something which is not on the page at all deserves one more try with the
/// failure spelled out. A delivered action is different again: it may have landed, and repeating it is
/// how the same thing gets done twice. Failures are therefore counted in two classes, and any progress
/// clears both — a run that is getting somewhere is not failing, whatever it stumbled over.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Miss {
    /// Nothing happened, and doing it again is safe.
    Retryable,
    /// Something may have happened. Never repeat this in place.
    Delivered,
}

#[derive(Default)]
struct Misses {
    retryable: u32,
    delivered: u32,
}

impl Misses {
    /// Note a failed step. True when the run has failed often enough to stop.
    fn note(&mut self, m: Miss) -> bool {
        match m {
            Miss::Retryable => self.retryable += 1,
            Miss::Delivered => self.delivered += 1,
        }
        self.retryable >= 2 || self.delivered >= 2
    }

    fn progress(&mut self) {
        self.retryable = 0;
        self.delivered = 0;
    }
}

/// How long a learning run may take, in wall-clock time, declared rather than discovered. Steps alone
/// are a poor budget: a single step can spend six seconds waiting for an address that never changes,
/// so fifty of those is five minutes of nothing.
const LEARN_BUDGET_SECS: u64 = 900;

async fn run_with_model(
    cdp: &mut Cdp,
    goal: &str,
    category: &str,
    max_steps: usize,
    rules: Option<policy::Policy>,
) -> Result<Run> {
    // Every run of this engine gets its own gate, made here rather than passed in, so there is no
    // call path that can reach an action without one.
    let run_host = skill::host_of_url(&cdp.current_url().await.unwrap_or_default());
    let mut gate = gate::Gate::new(&run_host);
    if let Some(rules) = rules {
        gate.adopt_policy(rules);
    }
    sign_for(&mut gate, &run_host);
    let started = Instant::now();
    let mut calls = 0u32;
    let mut trail: Vec<(Step, Element)> = Vec::new();
    let mut screens: Vec<Vec<Element>> = Vec::new();
    let mut log: Vec<String> = Vec::new();
    let mut declared_done = false;
    let mut misses = Misses::default();
    // Pages already visited. A run that comes back to one of them has stopped making
    // progress: everything after that point is wandering, and a skill compiled from it
    // would encode the wandering. The productive prefix ends at the first reversal.
    let mut seen: Vec<String> = vec![state::page_kind(&cdp.current_url().await?)];

    // How many times the model's claim of being finished has been sent back because the page
    // disagreed. Bounded: a model that insists deserves to be taken at its word eventually, and the
    // run has to end either way.
    let mut bounces = 0u32;
    for round in 0..max_steps {
        if started.elapsed().as_secs() > LEARN_BUDGET_SECS {
            println!(
                "  the run has taken {} minutes, the declared budget — stopping",
                LEARN_BUDGET_SECS / 60
            );
            break;
        }
        if std::env::var("MINT_DEBUG_STATE").is_ok() {
            println!("  [debug] iteration {} of {max_steps}, {} steps kept", round + 1, trail.len());
        }
        let (els, here) = cdp.state().await?;
        // MINT_DEBUG_STATE=2 prints what the model is given, which is the only way to tell a model
        // that cannot see its own work from one that sees it and ignores it.
        if std::env::var("MINT_DEBUG_STATE").as_deref() == Ok("2") {
            println!("  [state as the model reads it]\n{}", state::as_table(&els));
        }
        if std::env::var("MINT_DEBUG_STATE").is_ok() {
            println!("  [debug] {} elements on {}", els.len(), here);
        }
        screens.push(els.clone());
        let d = match decide::next_step(goal, &here, &els, &log, category) {
            Ok(d) => d,
            Err(e) => {
                // A reply that cannot be acted on is a step that did not happen, not the end of the
                // run. Over fifty steps a model gets one wrong, and throwing away the other
                // forty-nine because of it is how a long task is made impossible to learn.
                println!("  the model's answer could not be acted on: {e}");
                calls += 1;
                // A reply naming something that is not on the page is a wrong answer: worth one more
                // try with the failure spelled out, but waiting will not fix it.
                if misses.note(Miss::Retryable) {
                    println!("  two answers in a row could not be acted on — stopping");
                    break;
                }
                continue;
            }
        };
        calls += 1;
        let Some(idx) = d.idx else {
            // The model says the goal is reached. Over a form it says that while the fields it was
            // asked to fill are still empty — twice on the fifty-field form it declared the task
            // done at thirty-five of fifty, and the run was kept as a success. The page is the better
            // witness. The count is read from the same list the model was just shown, the claim is
            // sent back at most twice, and the number of empty fields goes into the log either way,
            // so the record says what actually happened.
            let empty = els.iter().filter(|e| state::is_empty_field(e)).count();
            if empty > 0 && bounces < 2 {
                bounces += 1;
                println!(
                    "   model: goal reached ({}) — but {empty} field(s) are still empty; sent back ({bounces} of 2)",
                    d.intent
                );
                log.push(format!(
                    "declared done with {empty} field(s) still empty — sent back ({bounces} of 2)"
                ));
                continue;
            }
            if empty > 0 {
                println!(
                    "   model: goal reached ({}) — accepted with {empty} field(s) still empty, after two bounces",
                    d.intent
                );
            } else {
                println!("   model: goal reached ({})", d.intent);
            }
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
            println!("  the gate did not allow this step: {why}");
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
                            // anything. And it is said out loud, for the same reason as above.
                            println!("  the gate did not allow the retry: {why}");
                            log.push(format!("the gate did not allow the retry — {why}"));
                            break;
                        }
                    }
                    None => acted = false,
                }
            }
        }
        if !acted {
            // An element that cannot be reached is a fact about the page, and the model can often
            // work around it — open the menu that holds it, or take another route. Stopping here
            // would throw away a run over something a person would simply click past.
            // Two very different failures used to be counted as one. An element that could not be
            // reached was not acted on at all, so trying again is safe. An element that was clicked
            // and left the page unchanged may have been clicked for real — the change may simply not
            // be visible yet — and repeating it is how the same thing gets done twice. The class goes
            // into the log too, so a run that ends here says which kind of failure ended it.
            let kind = if el.on { Miss::Delivered } else { Miss::Retryable };
            println!(
                "  the step did not happen ({}) — telling the model and going on",
                if el.on {
                    "the action was delivered and changed nothing visible, so it will not be repeated"
                } else {
                    "the element could not be reached from where it sits"
                }
            );
            log.push(format!(
                "failed: {} «{}» {}",
                el.role,
                el.name.chars().take(40).collect::<String>(),
                if el.on {
                    "was acted on and the page did not change"
                } else {
                    "could not be clicked from where it sits"
                }
            ));
            if misses.note(kind) {
                println!("  two failed actions in a row — stopping");
                break;
            }
            continue;
        }
        misses.progress();
        // How long to wait for the address to move depends on whether this action could move it.
        // Typing cannot, and waiting the full six seconds anyway is what made one field cost six
        // seconds: over fifty fields that is five minutes spent watching an address that was never
        // going to change. Whether a fill worked is checked against the field, not the address.
        let could_move = matches!(step.action, Action::Click)
            && el.role != "checkbox"
            && el.role != "radio";
        let moved = cdp
            .wait_for_url_change(&here, if could_move { 6000 } else { 700 })
            .await
            .unwrap_or_default();
        step.produced_page = state::page_kind(&moved);
        if std::env::var("MINT_DEBUG_STATE").is_ok() {
            println!(
                "  [debug] learned «{}» produced «{}» moved to «{}»",
                step.learned_page,
                step.produced_page,
                moved.chars().take(70).collect::<String>()
            );
        }
        // A page that has just arrived is usually still filling itself in — lists, tables, counts that
        // load a moment later. Deciding the next step against a half-built page aims it at elements
        // that are about to be replaced. Let it finish first.
        if step.produced_page != step.learned_page {
            let _ = cdp.wait_until_stable(6000).await;
        }
        // Filling a field changes the page's state without changing its address, and that is
        // progress of the plainest kind: a person filling in a form is doing the task. A runtime that
        // only counts navigation can never learn to fill anything in, which is most of what anybody
        // actually wants done. So a step counts if it either moved the page or the field took the
        // text — and the field is checked, not assumed, so a click that typed nothing still fails.
        if step.produced_page == step.learned_page {
            // A tick-box clicked with nothing to navigate to is still the task being done — and most
            // of what a form asks for is tick-boxes. Whether it moved is checked, not assumed, so a
            // click that missed the box still counts as a miss.
            if el.checked.is_some() && matches!(step.action, Action::Click) {
                // Read the box back through the same list the runtime already builds, so the element
                // is recognised exactly as it was recognised when it was clicked.
                let now = cdp
                    .state()
                    .await
                    .ok()
                    .and_then(|(els_now, _)| {
                        els_now
                            .iter()
                            .find(|e| e.role == el.role && e.name == el.name)
                            .and_then(|e| e.checked)
                    });
                if now.is_some() && now != el.checked {
                    println!(
                        "  set «{}» — the address did not change, but the box did",
                        el.name.chars().take(44).collect::<String>()
                    );
                    log.push(format!(
                        "set {} «{}» to {}",
                        el.role,
                        el.name.chars().take(40).collect::<String>(),
                        if now == Some(true) { "ticked" } else { "unticked" }
                    ));
                    trail.push((step, el.clone()));
                    continue;
                }
            }
            if let Action::Type { text } = &step.action {
                let want = text.trim().to_string();
                let holds = cdp.value_of(&el).await.unwrap_or_default().trim().to_string();
                if !want.is_empty() && holds == want {
                    println!(
                        "  filled «{}» — the address did not change, but the field did",
                        el.name.chars().take(44).collect::<String>()
                    );
                    log.push(format!(
                        "filled {} «{}» with «{}»",
                        el.role,
                        el.name.chars().take(40).collect::<String>(),
                        want.chars().take(30).collect::<String>()
                    ));
                    trail.push((step, el.clone()));
                    continue;
                }
            }
        }
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
        // An address the runtime has no kind for is not evidence of a reversal: two pages at that
        // depth are both named "", and cutting the run there would end a task that is going fine.
        if !step.produced_page.is_empty() && seen.contains(&step.produced_page) {
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
async fn run_skill(
    cdp: &mut Cdp,
    sk: &mut Skill,
    category: &str,
    rules: Option<policy::Policy>,
) -> Result<FastRun> {
    // Every run of this engine gets its own gate, made here rather than passed in, so there is no
    // call path that can reach an action without one.
    let run_host = skill::host_of_url(&cdp.current_url().await.unwrap_or_default());
    let mut gate = gate::Gate::new(&run_host);
    // Replaying runs under the shipped rules unless a document is given: the shipped ones hold
    // anything that commits, which is what makes a replay safe to leave alone. A task that writes has
    // to be replayed under a document that permits those writes, and the receipts then say so.
    if let Some(rules) = rules {
        gate.adopt_policy(rules);
    }
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
            let r = sk.resolve_explained(&step, &els);
            (r, t.elapsed().as_micros() as u64)
        } {
            (Resolution::One(idx, _conf, how), us) => {
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
            (Resolution::Ambiguous { why }, us) => {
                // Two candidates, or the place and the words pointing at different elements. Acting
                // here would be a guess with the runtime's name on it, so the step is refused and the
                // reason is said out loud — a person would stop and look, not click one of the two.
                notes.push(format!(
                    "  step {}: refused rather than guessed — {why}; asking the model",
                    i + 1
                ));
                escalated += 1;
                match recover(cdp, &goal, category, &mut log).await? {
                    Some(el) => (el, Found::Taught, us),
                    None => {
                        notes.push("  the model found no move either — stopping".into());
                        break;
                    }
                }
            }
            (Resolution::Unrecognised, us) => {
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
    // The rules a first run is learned under. Without one the shipped document is used, which holds
    // anything that commits — so a task that submits a form can be watched but never learned.
    let learn_rules = match flag(args, "--policy") {
        Some(path) => Some(policy::Policy::from_file(&path)?),
        None => None,
    };

    println!("FIRST RUN — the model decides every step\ngoal: {goal}\npage: {start}\n");
    match flag(args, "--policy") {
        Some(path) => println!("learned under the rules in {path}\n"),
        None => println!("learned under the shipped rules: anything that commits is held\n"),
    }
    if let Ok(closed) = Cdp::tidy_tabs(PORT).await {
        if closed > 0 {
            println!("  leftover tabs closed: {closed}\n");
        }
    }
    let mut cdp = Cdp::connect(PORT).await?;
    cdp.navigate(&start).await?;
    cdp.settle(4000).await?;
    // A page that is still building itself will swallow the first actions. Wait for the elements
    // to stop moving before deciding anything, rather than trusting that the document is loaded.
    let rounds = cdp.wait_until_stable(12_000).await.unwrap_or(0);
    println!("  the page settled after {rounds} looks\n");
    let run = run_with_model(&mut cdp, &goal, &category, max, learn_rules.clone()).await?;
    let title = cdp.title().await?;
    // The kind of page the *kept* steps end on — not wherever the wandering run stopped.
    // Falling back to the page the run *started* on put an expectation in the file that the run had
    // already left: the fifty-field task ends on /fieldwork/submit, whose address the runtime has no
    // kind for, so the skill expected the form page and every replay afterwards reported "landed
    // somewhere else" while landing exactly where it should. Name it by its last segment instead.
    let ended_on = run
        .trail
        .last()
        .map(|(st, _)| st.produced_page.clone())
        .unwrap_or_default();
    let final_kind = if !ended_on.is_empty() {
        ended_on
    } else {
        let deep = state::page_kind_deep(&cdp.current_url().await?);
        if deep.is_empty() { state::page_kind(&start) } else { deep }
    };
    let mut sk = skill::compile(&goal, &start, &run.trail, &run.screens, run.calls, run.ms, &final_kind);
    // A run that never declared success is not evidence that the skill works. Say so in
    // the file, so no later report can quietly claim otherwise.
    // Ask once whether the place we ended up actually satisfies the goal. The verdict is
    // about the outcome, not about our own recording of it.
    let body_text = cdp.text().await.unwrap_or_default();
    let here_now = cdp.current_url().await?;
    // The steps the runtime verified, in the words of what happened, for the check to read.
    let did: Vec<String> = run
        .trail
        .iter()
        .map(|(st, el)| match &st.action {
            Action::Type { text } => format!("typed «{text}» into {} «{}»", el.role, el.name),
            Action::Click => format!("clicked {} «{}»", el.role, el.name),
        })
        .collect();
    let verdict = decide::judge_success(&goal, &here_now, &title, &body_text, &did);
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
    // The rules the replay runs under. Without one, the shipped document is used and anything that
    // commits is held — which is the safe default and the reason a written task stops to ask.
    let run_rules = match flag(args, "--policy") {
        Some(p) => Some(policy::Policy::from_file(&p)?),
        None => None,
    };
    let mut sk = Skill::load(&path)?;

    println!("NEW DATA, THE SAME SKILL\n");
    match flag(args, "--policy") {
        Some(p) => println!("replay runs under the rules in {p}\n"),
        None => println!("replay runs under the shipped rules: anything that commits is held\n"),
    }
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
    // The same rules for both halves: a comparison where one side is allowed less than the other
    // is a comparison of permissions, not of agents.
    let slow_run = run_with_model(&mut slow, &sk.goal, &category, max, run_rules.clone()).await?;
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
    let r = run_skill(&mut fast, &mut sk, &category, run_rules.clone()).await?;
    for n in &r.notes {
        println!("{n}");
    }
    let fast_url = fast.current_url().await?;
    let fast_title = fast.title().await?;
    let reached = state::landed_on(&fast_url, &sk.expect_final_page);
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
    let demo_host = skill::host_of_url(&target);
    // The rules both halves run under, when the task names a document of its own.
    let rules = match flag(args, "--policy") {
        Some(p) => Some(policy::Policy::from_file(&p)?),
        None => None,
    };
    // Two gates, and the split is the point of the comparison. Both halves are bound by the same
    // rules — nothing here acts without being decided first — but only the compiled half records
    // what it decided, because a signed record of every step is the thing the other kind of agent
    // does not have. Giving both one log would make the count meaningless.
    let mut gate_slow = gate::Gate::new(&demo_host);
    let mut gate = gate::Gate::new(&demo_host);
    // Both halves run under the same document. A comparison where one side is allowed less than the
    // other compares permissions rather than agents — and the shipped rules hold anything that
    // commits, so a task that writes needs its own document named here.
    if let Some(rules) = &rules {
        gate_slow.adopt_policy(rules.clone());
        gate.adopt_policy(rules.clone());
    }
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
    // An answer this half cannot act on is a step it did not take, not the end of its run. Ending
    // there would report the comparison as a task the ordinary agent could not do at all, which
    // would be the harness's fault rather than the agent's — and over fifty steps a model will get
    // one wrong. Five is the ceiling: past that it is not bad luck.
    let mut unusable = 0u32;
    println!("\nORDINARY AGENT");
    for _ in 0..max {
        let (els, here) = slow.state().await?;
        let d = match decide::next_step(&goal, &here, &els, &log, &category) {
            Ok(d) => d,
            Err(e) => {
                println!("  the model's answer could not be acted on: {e}");
                unusable += 1;
                if unusable >= 5 {
                    println!("  five answers that could not be acted on — stopping");
                    break;
                }
                continue;
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
        if !perform(&mut slow, &mut gate_slow, &slow_step, n, &el, None).await?.0 {
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
    let slow_ok = state::landed_on(&slow_url, &sk.expect_final_page);
    let slow_verdict = format!(
        "landed on page «{}», expected «{}» — {}",
        slow_kind,
        sk.expect_final_page,
        if slow_ok { "matches" } else { "does NOT match" }
    );
    let slow_opinion = match decide::judge_success(&goal, &slow_url, &slow_title, &slow_body, &[]) {
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
    let fast_ok = state::landed_on(&fast_url, &sk.expect_final_page);
    let fast_verdict = format!(
        "landed on page «{}», expected «{}» — {}",
        fast_kind,
        sk.expect_final_page,
        if fast_ok { "matches" } else { "does NOT match" }
    );
    let fast_opinion = match decide::judge_success(&goal, &fast_url, &fast_title, &fast_body, &[]) {
        Ok((ok, why)) => format!("{}: {why}", if ok { "says it was reached" } else { "says it was not reached" }),
        Err(_) => String::new(),
    };
    println!("  → {:.2} s, {escalated} model calls, result: {fast_verdict}", fast_ms as f64 / 1000.0);
    push_event(&relay, "/push", json!({ "side": "fast", "finished": true, "total_ms": fast_ms,
        "url": fast_url, "page_title": fast_title, "verdict": fast_verdict, "opinion": fast_opinion }));

    // What the compiled half leaves behind, which is the half of the comparison that cannot be
    // argued with: a signed record, one decision at a time, checkable by whoever receives it.
    let mut record = json!({ "signed": false });
    if let Some(out) = flag(args, "--receipts") {
        if let Some(log) = gate.receipt_log() {
            let doc = log.document();
            std::fs::write(&out, serde_json::to_string_pretty(&doc)?)?;
            let rules_file = format!("{out}.policy.json");
            std::fs::write(&rules_file, serde_json::to_string_pretty(&log.rules)?)?;
            let v = receipt::verify(&doc, Some(&log.rules));
            record = json!({ "signed": true, "count": log.len(), "verified": v.ok,
                             "file": out, "policy_file": rules_file });
            println!("\nWHAT THE COMPILED HALF LEAVES BEHIND");
            println!("  {} signed receipts, written before each action — and they verify: {}",
                     log.len(), v.ok);
            println!("  the ordinary half leaves nothing of the kind: its steps were decided by a model");
            println!("  and no record of them was signed");
        }
    }
    push_event(&relay, "/push", json!({ "side": "fast", "receipts": record }));

    println!("\nTHE NUMBER FOR THE STAGE");
    println!("  ordinary agent: {:.1} s, {n} steps, every one a model call", slow_ms as f64 / 1000.0);
    println!("  mint:         {:.2} s, {done} steps, model calls {escalated}", fast_ms as f64 / 1000.0);
    println!("  the same task, the same page, and the difference is not the clock:");
    println!("  the ordinary agent pays {n} questions to a model EVERY time this task is run.");
    println!("  mint paid them once, when the task was learned, and pays {escalated} now.");
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
    let ok = state::landed_on(&landed, &sk.expect_final_page);
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


#[cfg(test)]
mod person_command_tests {
    use super::*;

    #[test]
    fn the_agents_own_approval_command_is_recognised() {
        for c in [
            "mint approve --action guard:abc --days 1",
            "~/.local/bin/mint approve --action guard:abc",
            "MINT_BIN=/usr/local/bin/mint /usr/local/bin/mint approve --days 1",
            "/root/mint/skills/mint/scripts/approve.sh --action guard:abc",
            "mint revoke --action guard:abc",
            "cd /tmp && mint approve --days 1",
        ] {
            assert!(person_command(c).is_some(), "not recognised: {c}");
        }
    }

    #[test]
    fn ordinary_work_is_not_mistaken_for_it() {
        for c in [
            "ls -la",
            "git commit -m 'x'",
            "mint guard --command \"git commit\"",
            "mint hook",
            "mint receipts .mint/guard.json",
            "cat approval.json",
            "grep -r approve src/",
        ] {
            assert!(person_command(c).is_none(), "wrongly refused: {c}");
        }
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn two_different_commands_are_two_different_actions() {
        let commit = shell::observed("git commit -m 'x'", false);
        let push = shell::observed("git push origin main", false);
        // Both are writes, and the observation says exactly the same thing about both of them — which is
        // precisely why the name has to come from the command.
        assert_eq!(
            serde_json::to_string(&commit).unwrap(),
            serde_json::to_string(&push).unwrap()
        );
        assert_ne!(
            action_identity(&commit, Some("git commit -m 'x'")),
            action_identity(&push, Some("git push origin main")),
            "an approval for one of these must not authorise the other"
        );
    }

    #[test]
    fn the_same_command_twice_is_the_same_action() {
        let a = shell::observed("git push origin main", false);
        let b = shell::observed("git push origin main", false);
        assert_eq!(
            action_identity(&a, Some("git push origin main")),
            action_identity(&b, Some("git push origin main")),
            "single use depends on this being stable"
        );
    }

    #[test]
    fn an_action_with_no_command_is_named_by_its_observation_alone() {
        let el = policy::Observed {
            action: "click".into(),
            tag: "button".into(),
            role: "button".into(),
            has_href: false,
            approval: false,
        };
        assert_eq!(action_identity(&el, None), action_identity(&el, None));
        assert_ne!(action_identity(&el, None), action_identity(&el, Some("git push")));
    }
}
