//! 职责：
//! - 提供无头的 JSON 命令行入口
//! - 默认连接运行中的桌面会话，`--local` 时使用本进程运行时
//! - 支持单条命令、stdin 批量与 describe
//!
//! 禁止：
//! - 在 CLI 层实现业务逻辑

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::doc_markdown,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::unnecessary_wraps
)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

use std::io::{self, BufRead};
use std::process::ExitCode;

use aurtrack_command::CommandRequest;
use aurtrack_ipc::Backend;

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        },
    }
}

fn run() -> Result<ExitCode, String> {
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let force_local = raw_args.iter().any(|arg| arg == "--local");
    let args: Vec<String> = raw_args
        .into_iter()
        .filter(|arg| arg != "--local")
        .collect();

    let Some(first) = args.first() else {
        print_help();
        return Ok(ExitCode::SUCCESS);
    };

    let mut backend = if force_local {
        eprintln!("使用本地运行时");
        Backend::local()
    } else if let Ok(backend) = Backend::remote() {
        eprintln!("已连接运行中的 Aurtrack");
        backend
    } else {
        eprintln!("没有运行中的 Aurtrack，使用本地运行时");
        Backend::local()
    };

    match first.as_str() {
        "--help" | "-h" => {
            print_help();
            Ok(ExitCode::SUCCESS)
        },
        "--json" => {
            let raw = args
                .get(1)
                .ok_or_else(|| "--json 需要一个 JSON 参数".to_string())?;
            let request = parse_request(raw)?;
            let (line, ok) = execute(&mut backend, &request);
            println!("{line}");
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        },
        "--stdin" => run_stdin(&mut backend, args.iter().any(|arg| arg == "--hold")),
        command => {
            let request = match args.get(1) {
                None => CommandRequest {
                    cmd: command.to_string(),
                    args: serde_json::json!({}),
                },
                Some(flag) if flag == "--args" => {
                    let raw = args
                        .get(2)
                        .ok_or_else(|| "--args 需要一个 JSON 参数".to_string())?;
                    let parsed = serde_json::from_str(raw)
                        .map_err(|error| format!("参数不是合法 JSON：{error}"))?;
                    CommandRequest {
                        cmd: command.to_string(),
                        args: parsed,
                    }
                },
                Some(other) => return Err(format!("无法识别的参数：{other}")),
            };
            let (line, ok) = execute(&mut backend, &request);
            println!("{line}");
            Ok(if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        },
    }
}

fn run_stdin(backend: &mut Backend, hold: bool) -> Result<ExitCode, String> {
    let stdin = io::stdin();
    let mut failed = false;
    for line in stdin.lock().lines() {
        let line = line.map_err(|error| format!("读取 stdin 失败：{error}"))?;
        if line.trim().is_empty() {
            continue;
        }
        let request = parse_request(&line)?;
        let (output, ok) = execute(backend, &request);
        println!("{output}");
        if !ok {
            failed = true;
        }
    }
    if hold && !backend.is_remote() {
        eprintln!("播放中，按 Ctrl+C 退出。");
        loop {
            std::thread::park();
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn parse_request(raw: &str) -> Result<CommandRequest, String> {
    serde_json::from_str(raw).map_err(|error| format!("不是合法的命令 JSON：{error}"))
}

fn execute(backend: &mut Backend, request: &CommandRequest) -> (String, bool) {
    match backend.execute(request) {
        Ok(response) => {
            let text = serde_json::to_string(&response)
                .unwrap_or_else(|error| error_json("internal_error", &error.to_string()));
            (text, true)
        },
        Err(error) => (error_json(&error.code, &error.message), false),
    }
}

fn error_json(code: &str, message: &str) -> String {
    serde_json::json!({
        "ok": false,
        "error": { "code": code, "message": message }
    })
    .to_string()
}

fn print_help() {
    println!("Aurtrack CLI");
    println!();
    println!("用法：");
    println!("  aurtrack --json '{{\"cmd\":\"describe\",\"args\":{{}}}}'");
    println!("  aurtrack describe");
    println!("  aurtrack track.create --args '{{\"name\":\"bass\"}}'");
    println!("  aurtrack --stdin");
    println!("  aurtrack --stdin --hold   # 本地运行时读完后保持进程");
    println!("  aurtrack --local describe # 不连接桌面，使用本进程运行时");
}
