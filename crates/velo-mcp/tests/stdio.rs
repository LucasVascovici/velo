//! Spawns the real binary and checks that stdout carries protocol lines only.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::Value;

#[test]
fn stdout_is_json_rpc_only() {
    let dir = tempfile::tempdir().unwrap();
    velo_core::Repo::init(dir.path()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_velo-mcp"))
        .args(["--repo", dir.path().to_str().unwrap(), "--run", "test-run"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","clientInfo":{{"name":"t"}}}}}}"#
        )
        .unwrap();
        writeln!(
            stdin,
            r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
        )
        .unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list"}}"#).unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).expect("every stdout line is JSON"))
        .collect();
    assert_eq!(lines.len(), 2);
    for (i, v) in lines.iter().enumerate() {
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], i as u64 + 1);
    }
    assert_eq!(lines[1]["result"]["tools"].as_array().unwrap().len(), 6);
}
