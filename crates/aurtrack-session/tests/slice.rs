//! 第一条竖切的端到端测试：建工程、建轨、加设备、调参、渲染、撤销。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aurtrack_command::CommandRequest;
use aurtrack_session::Session;
use serde_json::{Value, json};

fn run(session: &mut Session, cmd: &str, args: Value) -> Value {
    session
        .execute(&CommandRequest {
            cmd: cmd.to_string(),
            args,
        })
        .unwrap()
        .data
}

fn temp_path(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!("aurtrack-{tag}-{}.wav", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn full_slice_produces_audio_and_undo_restores_it() {
    let mut session = Session::default();

    let project = run(&mut session, "project.create", json!({ "name": "demo" }));
    assert_eq!(project["project"]["name"], "demo");

    let track = run(
        &mut session,
        "track.create",
        json!({ "name": "bass", "source": "sine", "frequency": 440.0 }),
    );
    let track_id = track["track_id"].as_str().unwrap().to_string();

    let device = run(
        &mut session,
        "device.add",
        json!({ "track_id": track_id, "kind": "gain" }),
    );
    let device_id = device["device_id"].as_str().unwrap().to_string();
    let path = format!("/tracks/{track_id}/devices/{device_id}/params/gain");

    run(
        &mut session,
        "param.set",
        json!({ "path": path, "value": 0.5 }),
    );
    let normal = temp_path("normal");
    let out = run(
        &mut session,
        "render.segment",
        json!({ "duration_seconds": 0.1, "path": normal }),
    );
    assert!(out["peak"].as_f64().unwrap() > 0.0);
    assert!(std::fs::metadata(&normal).unwrap().len() > 44);

    run(
        &mut session,
        "param.set",
        json!({ "path": path, "value": 0.0 }),
    );
    let muted = temp_path("muted");
    let out = run(
        &mut session,
        "render.segment",
        json!({ "duration_seconds": 0.1, "path": muted }),
    );
    assert!(out["peak"].as_f64().unwrap().abs() < 1e-12);

    run(&mut session, "undo", json!({}));
    let restored = temp_path("restored");
    let out = run(
        &mut session,
        "render.segment",
        json!({ "duration_seconds": 0.1, "path": restored }),
    );
    assert!(out["peak"].as_f64().unwrap() > 0.0);
}

#[test]
fn describe_lists_sixteen_commands() {
    let mut session = Session::default();
    let out = run(&mut session, "describe", json!({}));
    assert_eq!(out["commands"].as_array().unwrap().len(), 16);
    assert!(out["project"].is_null());
}

#[test]
fn remove_track_supports_undo_and_redo() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "t" }));
    run(
        &mut session,
        "track.remove",
        json!({ "track_id": "track-1" }),
    );

    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(state["project"]["tracks"].as_array().unwrap().len(), 0);

    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(state["project"]["tracks"].as_array().unwrap().len(), 1);

    run(&mut session, "redo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(state["project"]["tracks"].as_array().unwrap().len(), 0);
}

#[test]
fn track_before_project_is_not_found() {
    let mut session = Session::default();
    let error = session
        .execute(&CommandRequest {
            cmd: "track.create".to_string(),
            args: json!({ "name": "x" }),
        })
        .unwrap_err();
    assert_eq!(error.code, "not_found");
}
