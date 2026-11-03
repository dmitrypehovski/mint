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
