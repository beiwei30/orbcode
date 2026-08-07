use std::io::{self, BufRead, Write};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    write_argv_log(&args);
    let target = args
        .iter()
        .position(|argument| argument == "--")
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
        .unwrap_or_default();

    match target {
        "host-key.example" => {
            eprintln!("Host key verification failed.");
            std::process::exit(255);
        }
        "auth.example" => {
            eprintln!("user@auth.example: Permission denied (publickey,password).");
            std::process::exit(255);
        }
        "missing.example" => {
            eprintln!("orbcode: command not found");
            std::process::exit(127);
        }
        "connection.example" => {
            eprintln!(
                "ssh: Could not resolve hostname connection.example: Name or service not known"
            );
            std::process::exit(255);
        }
        "unknown-ssh.example" => {
            eprintln!("ssh: unexplained test failure");
            std::process::exit(255);
        }
        "hang.example" => {
            thread::sleep(Duration::from_secs(30));
            return;
        }
        _ => {}
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.expect("read stdin");
        if target == "protocol.example" {
            writeln!(stdout, "{{malformed-protocol}}").expect("write malformed protocol");
            stdout.flush().expect("flush malformed protocol");
            continue;
        }
        let message: Value = serde_json::from_str(&line).expect("canonical client message");
        let id = message["id"].as_str().expect("request id");
        match message["method"].as_str().expect("request method") {
            "initialize" => respond(
                &mut stdout,
                id,
                json!({
                    "protocol_version": "1.0",
                    "server_info": {"name": "fake-ssh-orbcode", "version": "0.1.0"},
                    "capabilities": {
                        "streaming": true,
                        "stable_methods": ["session/list"],
                        "experimental_methods": [],
                        "server_notification_methods": [],
                        "server_request_methods": ["permission/request", "mcp/trust", "ask_user/question"]
                    }
                }),
            ),
            "session/list" if target == "pending.example" => {
                write_message(
                    &mut stdout,
                    json!({
                        "type": "request",
                        "id": "pending-server-request",
                        "method": "permission/request",
                        "params": {
                            "request_id": "pending-permission",
                            "session_id": "pending-session",
                            "tool_use_id": "pending-tool-use",
                            "tool_name": "Bash",
                            "tool_input": "{}",
                            "requires_tools_permission": true,
                            "requires_network_permission": false
                        }
                    }),
                );
            }
            "session/list" => respond(&mut stdout, id, json!([])),
            method => panic!("unexpected method: {method}"),
        }
    }
}

fn write_argv_log(args: &[String]) {
    let Some(path) = args
        .windows(2)
        .find(|pair| pair[0] == "-o" && pair[1].starts_with("IdentityFile="))
        .map(|pair| pair[1].trim_start_matches("IdentityFile="))
    else {
        return;
    };
    std::fs::write(path, serde_json::to_vec(args).expect("serialize argv"))
        .expect("write argv log");
}

fn respond(stdout: &mut impl Write, id: &str, data: Value) {
    write_message(
        stdout,
        json!({"type": "response", "id": id, "result": {"status": "success", "data": data}}),
    );
}

fn write_message(stdout: &mut impl Write, message: Value) {
    serde_json::to_writer(&mut *stdout, &message).expect("serialize response");
    writeln!(stdout).expect("terminate response");
    stdout.flush().expect("flush response");
}
