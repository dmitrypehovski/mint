//! The hook, as the harness calls it: a payload in, a decision out.
//!
//! These run the real binary with a real payload on stdin, because that is the only interface that
//! matters — the harness does not call a function, it runs a command and reads its answer.

use std::io::Write;
use std::process::{Command, Stdio};

fn hook(payload: &str, receipts: &str) -> (String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mint"))
        .args(["hook", "--receipts", receipts])
        .env("MINT_RECEIPT_KEY", format!("{receipts}/key"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary runs");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn dir(tag: &str) -> String {
    let path = format!("/tmp/mint-hook-{tag}-{}", std::process::id());
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn payload(command: &str, cwd: &str) -> String {
    serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "cwd": cwd,
        "tool_input": { "command": command }
    })
    .to_string()
}

#[test]
fn a_read_is_allowed_without_a_word() {
    let d = dir("read");
    let (out, code) = hook(&payload("ls -la", &d), &d);
    assert_eq!(code, 0, "the answer travels in the payload, not the exit status");
    assert_eq!(out.trim(), "{}", "an empty answer is permission");
}

#[test]
fn a_commit_is_denied_before_it_runs_and_the_denial_names_what_approves_it() {
    let d = dir("commit");
    let (out, code) = hook(&payload("git commit -m 'ship it'", &d), &d);
    assert_eq!(code, 0);
    let doc: serde_json::Value = serde_json::from_str(&out).expect("the harness reads json");
    let decision = &doc["hookSpecificOutput"];
    assert_eq!(decision["permissionDecision"], "deny");
    let reason = decision["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.contains("held"), "the reason says what happened: {reason}");
    assert!(
        reason.contains("mint approve --action guard:"),
        "and gives the command that approves this exact action: {reason}"
    );

    // A hold that is not in the log did not happen: the person who has to act on it reads the log.
    let log: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{d}/guard.json")).unwrap()).unwrap();
    let receipts = log["receipts"].as_array().unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["body"]["decision"]["verdict"], "hold");
    assert_eq!(receipts[0]["body"]["action"]["side_effect"], "write");
}

#[test]
fn an_agent_cannot_approve_its_own_action() {
    // The whole mechanism rests on this: if the agent can lift the hold, the gate is a formality. The
    // hook is the surface we control, because every shell command the agent runs passes through it.
    let d = dir("selfapprove");
    for command in [
        "mint approve --action guard:abc --days 1 --out approval.json",
        "~/.local/bin/mint approve --days 1",
        "bash skills/mint/scripts/approve.sh --action guard:abc",
        "mint revoke --action guard:abc",
    ] {
        let (out, code) = hook(&payload(command, &d), &d);
        assert_eq!(code, 0);
        let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            doc["hookSpecificOutput"]["permissionDecision"], "deny",
            "not denied: {command}"
        );
        let reason = doc["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("person's act"), "the reason says whose act it is: {reason}");
    }
    // And ordinary work is still judged by the rules rather than refused for the wrong reason: a read
    // passes, and the refusal above is specifically about whose act the command is.
    let (out, _) = hook(&payload("git status --short", &d), &d);
    assert_eq!(out.trim(), "{}", "a read is still allowed: {out}");
    // `mint receipts` is a different case and worth knowing about: the shell adapter does not recognise it
    // as a read, so it is refused as unclassifiable rather than waved through. Strict, and it means the
    // way to read a log from inside a session is to read the file.
    let (out, _) = hook(&payload("cat .mint/guard.json", &d), &d);
    assert_eq!(out.trim(), "{}", "reading the log itself is a read: {out}");
}

#[test]
fn the_first_call_in_a_project_creates_its_own_directory() {
    // The lock is a file inside the log's directory, and on the very first call there is no directory yet.
    // Getting that wrong made every command in a fresh project fail closed, which is safe and useless.
    let d = format!("/tmp/mint-hook-first-{}", std::process::id());
    let _ = std::fs::remove_dir_all(&d);
    let nested = format!("{d}/does/not/exist/yet");
    let (out, code) = hook(&payload("git status --short", &nested), &nested);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "{}", "a read in a fresh project must be allowed: {out}");
    let (out, _) = hook(&payload("git commit -m 'x'", &nested), &nested);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["hookSpecificOutput"]["permissionDecision"], "deny", "and a write still held");
    assert!(
        std::path::Path::new(&format!("{nested}/guard.json")).exists(),
        "the log was created where the command was run"
    );
}

#[test]
fn a_lock_left_behind_by_a_dead_process_does_not_block_everything() {
    // The gate exits with `process::exit`, which runs no destructors, so a lock file can outlive its
    // holder. A lock that is never released refuses every command forever, and the person switches the gate
    // off — worse than the race it was added to fix. A pid that is not running is not a holder.
    let d = dir("stalelock");
    std::fs::write(format!("{d}/guard.json.lock"), "999999999").unwrap();
    let (out, code) = hook(&payload("git status --short", &d), &d);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "{}", "a dead holder must not hold the lock: {out}");
    assert!(
        !std::path::Path::new(&format!("{d}/guard.json.lock")).exists(),
        "and the lock it took is released"
    );
}

#[test]
fn decisions_taken_at_the_same_time_all_reach_the_log() {
    // The log is rewritten in full to append to it, so two hooks that read before either writes lose one
    // decision — and the lost one is invisible: the command was still refused, and nothing says anybody
    // was asked. Codex issues commands in parallel where Claude Code issued them one at a time, which is
    // how this was found.
    let d = dir("concurrent");
    let commands: Vec<String> = (0..8).map(|i| format!("git commit -m 'change {i}'")).collect();
    let handles: Vec<_> = commands
        .iter()
        .map(|c| {
            let c = c.clone();
            let d = d.clone();
            std::thread::spawn(move || hook(&payload(&c, &d), &d))
        })
        .collect();
    for h in handles {
        let (_, code) = h.join().expect("the hook finishes");
        assert_eq!(code, 0);
    }
    let log: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{d}/guard.json")).unwrap()).unwrap();
    let receipts = log["receipts"].as_array().unwrap();
    assert_eq!(
        receipts.len(),
        commands.len(),
        "every decision must be in the log, and none may overwrite another: {} of {}",
        receipts.len(),
        commands.len()
    );
    // And the log they produced still verifies: a lock that preserved receipts by breaking the chain
    // would be worse than the race.
    let out = Command::new(env!("CARGO_BIN_EXE_mint"))
        .args(["receipts", "--in", &format!("{d}/guard.json")])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("OK —"), "the log must still verify: {text}");
}

#[test]
fn a_payload_that_cannot_be_read_is_denied_rather_than_waved_through() {
    // The permissive direction is the one failure this file exists to avoid: a call nobody could judge
    // must not run.
    let d = dir("broken");
    let (out, code) = hook("this is not json", &d);
    assert_eq!(code, 0);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["hookSpecificOutput"]["permissionDecision"], "deny");
}

#[test]
fn a_second_call_of_a_released_command_is_denied_from_the_log() {
    let d = dir("spent");
    let command = "git commit -m 'ship it'";
    let (out, _) = hook(&payload(command, &d), &d);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let reason = doc["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    let identity = reason
        .split("--action ")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .expect("the reason names the action");

    // Approve it the way a person would, then release it once through the guard, then ask again.
    let approval = format!("{d}/approval.json");
    let status = Command::new(env!("CARGO_BIN_EXE_mint"))
        .args(["approve", "--action", identity, "--days", "1", "--out", &approval])
        // The same runtime key as the hook and the guard: an approval is bound to the runtime it was
        // issued for, and a test that issues it for a different one proves nothing.
        .env("MINT_APPROVER_KEY", format!("{d}/approver.key"))
        .env("MINT_RECEIPT_KEY", format!("{d}/key"))
        .status()
        .unwrap();
    assert!(status.success(), "the approval is issued");
    let kid: String = {
        let doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&approval).unwrap()).unwrap();
        doc["sig"]["kid"].as_str().unwrap().to_string()
    };
    let approvers = format!("{d}/approvers.json");
    std::fs::write(
        &approvers,
        serde_json::json!({ "approvers": [{ "key": kid, "name": "dmitry" }] }).to_string(),
    )
    .unwrap();

    let released = Command::new(env!("CARGO_BIN_EXE_mint"))
        .args([
            "guard", "--command", command, "--approval", &approval,
            "--approvers", &approvers, "--receipts", &d,
        ])
        .env("MINT_RECEIPT_KEY", format!("{d}/key"))
        .status()
        .unwrap();
    assert_eq!(released.code(), Some(0), "the approval releases the action once");

    let (out, _) = hook(&payload(command, &d), &d);
    let doc: serde_json::Value = serde_json::from_str(&out).unwrap();
    let reason = doc["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    assert!(
        reason.contains("already released once"),
        "the log remembers, so the hook does too: {reason}"
    );
}
