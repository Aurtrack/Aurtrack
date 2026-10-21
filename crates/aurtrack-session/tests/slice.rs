//! 第一条竖切的端到端测试：建工程、建轨、加设备、调参、渲染、撤销。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

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

fn project_temp_path(tag: &str) -> String {
    std::env::temp_dir()
        .join(format!(
            "aurtrack-{tag}-{}.aurtrack.json",
            std::process::id()
        ))
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
fn duplicate_and_quantize_notes_and_clips() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    run(
        &mut session,
        "clip.create",
        json!({ "track_id": "track-1", "start": 0.0, "duration": 2.0 }),
    );
    let note_path = "/tracks/track-1/clips/clip-1/notes/note-1";
    run(
        &mut session,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "pitch": 60,
            "start": 0.27,
            "duration": 0.5,
            "velocity": 0.8,
        }),
    );

    let duplicate = run(&mut session, "note.duplicate", json!({ "path": note_path }));
    assert_ne!(duplicate["note_id"], "note-1");
    assert!((duplicate["note"]["start"].as_f64().unwrap() - 0.77).abs() < 1e-9);

    let quantized = run(
        &mut session,
        "note.quantize",
        json!({ "path": note_path, "grid": 0.25 }),
    );
    assert!((quantized["note"]["start"].as_f64().unwrap() - 0.25).abs() < 1e-9);

    let clip = run(
        &mut session,
        "clip.duplicate",
        json!({ "path": "/tracks/track-1/clips/clip-1" }),
    );
    assert_eq!(clip["clip_id"], "clip-2");
    assert!((clip["clip"]["start"].as_f64().unwrap() - 2.0).abs() < 1e-9);
    assert_eq!(clip["clip"]["notes"].as_array().unwrap().len(), 2);

    let state = run(&mut session, "project.state", json!({}));
    let clips = state["project"]["tracks"][0]["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 2);
    let copied = clips
        .iter()
        .find(|clip| clip["id"] == "clip-2")
        .expect("copy exists");
    let ids = copied["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["id"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 2);
}

#[test]
fn paste_notes_and_clip_in_one_snapshot() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    run(
        &mut session,
        "clip.create",
        json!({ "track_id": "track-1", "start": 0.0, "duration": 2.0 }),
    );

    let pasted = run(
        &mut session,
        "note.paste",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "notes": [
                { "pitch": 60, "start": 0.0, "duration": 0.25, "velocity": 0.8 },
                { "pitch": 64, "start": 0.25, "duration": 0.25 }
            ],
        }),
    );
    assert_eq!(pasted["note_ids"].as_array().unwrap().len(), 2);
    assert_eq!(pasted["notes"][1]["velocity"], 0.8);

    let pasted_clip = run(
        &mut session,
        "clip.paste",
        json!({
            "track_id": "track-1",
            "name": "copy",
            "start": 2.0,
            "duration": 1.0,
            "notes": [{ "pitch": 67, "start": 0.0, "duration": 0.5 }],
        }),
    );
    assert_eq!(pasted_clip["clip_id"], "clip-2");
    assert_eq!(pasted_clip["clip"]["notes"].as_array().unwrap().len(), 1);

    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["clips"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn describe_lists_twenty_nine_commands() {
    let mut session = Session::default();
    let out = run(&mut session, "describe", json!({}));
    assert_eq!(out["commands"].as_array().unwrap().len(), 29);
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
fn project_save_and_open_round_trip() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "saved" }));
    run(
        &mut session,
        "track.create",
        json!({ "name": "bass", "source": "sine", "frequency": 110.0 }),
    );
    run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "gain" }),
    );
    run(
        &mut session,
        "param.set",
        json!({
            "path": "/tracks/track-1/devices/device-1/params/gain",
            "value": 0.5
        }),
    );
    run(
        &mut session,
        "clip.create",
        json!({
            "track_id": "track-1",
            "name": "verse",
            "start": 0.0,
            "duration": 2.0
        }),
    );
    run(
        &mut session,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "pitch": 69,
            "start": 0.25,
            "duration": 0.5,
            "velocity": 0.8
        }),
    );

    let path = project_temp_path("round-trip");
    let saved = run(&mut session, "project.save", json!({ "path": path }));
    assert!(saved["bytes"].as_u64().unwrap() > 0);

    let mut opened = Session::default();
    let response = run(&mut opened, "project.open", json!({ "path": path }));
    assert_eq!(response["project"]["name"], "saved");

    let state = run(&mut opened, "project.state", json!({}));
    assert_eq!(state["project"]["tracks"][0]["source"]["frequency"], 110.0);
    assert_eq!(
        state["project"]["tracks"][0]["devices"][0]["params"]["gain"],
        0.5
    );
    assert_eq!(state["project"]["tracks"][0]["clips"][0]["id"], "clip-1");
    assert_eq!(
        state["project"]["tracks"][0]["clips"][0]["notes"][0]["pitch"],
        69
    );

    let next = run(&mut opened, "track.create", json!({ "name": "next" }));
    assert_eq!(next["track_id"], "track-2");

    let clip = run(
        &mut opened,
        "clip.create",
        json!({
            "track_id": "track-1",
            "name": "next",
            "start": 2.0,
            "duration": 1.0
        }),
    );
    assert_eq!(clip["clip_id"], "clip-2");

    let note = run(
        &mut opened,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-2",
            "pitch": 72,
            "start": 0.0,
            "duration": 0.25
        }),
    );
    assert_eq!(note["note_id"], "note-2");

    run(&mut opened, "undo", json!({ "steps": 4 }));
    let state = run(&mut opened, "project.state", json!({}));
    assert!(state["project"].is_null());

    std::fs::remove_file(path).unwrap();
}

#[test]
fn clip_and_note_commands_validate_and_undo() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "notes" }));
    run(
        &mut session,
        "track.create",
        json!({ "name": "lead", "source": "silence" }),
    );

    let clip = run(
        &mut session,
        "clip.create",
        json!({
            "track_id": "track-1",
            "name": "verse",
            "start": 0.0,
            "duration": 2.0
        }),
    );
    assert_eq!(clip["clip_id"], "clip-1");

    let note = run(
        &mut session,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "pitch": 69,
            "start": 0.25,
            "duration": 0.5
        }),
    );
    assert_eq!(note["note_id"], "note-1");

    let error = session
        .execute(&CommandRequest {
            cmd: "clip.update".to_string(),
            args: json!({
                "path": "/tracks/track-1/clips/clip-1",
                "duration": 0.25
            }),
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid_args");

    let updated_clip = run(
        &mut session,
        "clip.update",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "name": "chorus",
            "start": 0.5,
            "duration": 2.0
        }),
    );
    assert_eq!(updated_clip["clip"]["name"], "chorus");

    let updated_note = run(
        &mut session,
        "note.update",
        json!({
            "path": "/tracks/track-1/clips/clip-1/notes/note-1",
            "pitch": 72,
            "start": 0.5,
            "duration": 0.5,
            "velocity": 0.5
        }),
    );
    assert_eq!(updated_note["note"]["pitch"], 72);

    let error = session
        .execute(&CommandRequest {
            cmd: "note.add".to_string(),
            args: json!({
                "path": "/tracks/track-1/clips/clip-1",
                "pitch": 72,
                "start": 1.8,
                "duration": 0.5
            }),
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid_args");

    run(
        &mut session,
        "note.remove",
        json!({ "path": "/tracks/track-1/clips/clip-1/notes/note-1" }),
    );
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["clips"][0]["notes"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["clips"][0]["notes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let wav = temp_path("note-render");
    let rendered = run(
        &mut session,
        "render.segment",
        json!({ "duration_seconds": 2, "path": wav }),
    );
    assert!(rendered["peak"].as_f64().unwrap() > 0.0);
    std::fs::remove_file(wav).unwrap();
}

#[test]
fn project_open_rejects_old_format_version() {
    let path = project_temp_path("bad-version");
    std::fs::write(
        &path,
        r#"{"format_version":1,"project":{"name":"x","sample_rate":48000,"tracks":[]}}"#,
    )
    .unwrap();

    let mut session = Session::default();
    let error = session
        .execute(&CommandRequest {
            cmd: "project.open".to_string(),
            args: json!({ "path": path }),
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid_args");

    std::fs::remove_file(path).unwrap();
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
