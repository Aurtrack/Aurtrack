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
    assert!((duplicate["note"]["start"].as_f64().unwrap() - 0.77).abs() < 1e-3);

    let quantized = run(
        &mut session,
        "note.quantize",
        json!({ "path": note_path, "grid": 0.25 }),
    );
    assert!((quantized["note"]["start"].as_f64().unwrap() - 0.25).abs() < 1e-3);

    let clip = run(
        &mut session,
        "clip.duplicate",
        json!({ "path": "/tracks/track-1/clips/clip-1" }),
    );
    assert_eq!(clip["clip_id"], "clip-2");
    assert!((clip["clip"]["start"].as_f64().unwrap() - 2.0).abs() < 1e-3);
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
fn batch_note_edits_and_cross_track_clip_move_share_snapshots() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    run(&mut session, "track.create", json!({ "name": "pad" }));
    run(
        &mut session,
        "clip.create",
        json!({ "track_id": "track-1", "start": 0.0, "duration": 2.0 }),
    );
    for (pitch, start) in [(60, 0.27), (64, 0.61), (67, 1.3)] {
        run(
            &mut session,
            "note.add",
            json!({
                "path": "/tracks/track-1/clips/clip-1",
                "pitch": pitch,
                "start": start,
                "duration": 0.2,
            }),
        );
    }

    let quantized = run(
        &mut session,
        "note.quantize_many",
        json!({
            "paths": [
                "/tracks/track-1/clips/clip-1/notes/note-1",
                "/tracks/track-1/clips/clip-1/notes/note-2"
            ],
            "grid": 0.25
        }),
    );
    assert_eq!(quantized["count"], 2);
    let state = run(&mut session, "project.state", json!({}));
    let starts = state["project"]["tracks"][0]["clips"][0]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["start"].as_f64().unwrap())
        .collect::<Vec<_>>();
    assert!((starts[0] - 0.25).abs() < 1e-3);
    assert!((starts[1] - 0.5).abs() < 1e-3);

    run(&mut session, "undo", json!({}));
    let error = session
        .execute(&CommandRequest {
            cmd: "note.remove_many".to_string(),
            args: json!({
                "paths": [
                    "/tracks/track-1/clips/clip-1/notes/note-1",
                    "/tracks/track-1/clips/clip-1/notes/note-404"
                ]
            }),
        })
        .unwrap_err();
    assert_eq!(error.code, "not_found");

    let removed = run(
        &mut session,
        "note.remove_many",
        json!({
            "paths": [
                "/tracks/track-1/clips/clip-1/notes/note-1",
                "/tracks/track-1/clips/clip-1/notes/note-2"
            ]
        }),
    );
    assert_eq!(removed["removed"], 2);
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["clips"][0]["notes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    run(&mut session, "undo", json!({}));
    let moved = run(
        &mut session,
        "clip.move",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "track_id": "track-2",
            "start": 1.0
        }),
    );
    assert_eq!(moved["path"], "/tracks/track-2/clips/clip-1");
    assert!((moved["clip"]["start"].as_f64().unwrap() - 1.0).abs() < 1e-3);
    assert_eq!(moved["clip"]["notes"].as_array().unwrap().len(), 3);

    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["clips"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        state["project"]["tracks"][1]["clips"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["clips"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        state["project"]["tracks"][1]["clips"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn describe_lists_fifty_nine_commands() {
    let mut session = Session::default();
    let out = run(&mut session, "describe", json!({}));
    assert_eq!(out["commands"].as_array().unwrap().len(), 59);
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
    assert_eq!(
        state["project"]["tracks"][0]["graph"]["nodes"]["instrument"]["params"]["frequency"],
        110.0
    );
    assert_eq!(
        state["project"]["tracks"][0]["graph"]["connections"]
            .as_array()
            .unwrap()
            .len(),
        2
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
fn adsr_device_params_are_validated_and_undoable() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));

    let device = run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "amp.adsr" }),
    );
    let device_id = device["device_id"].as_str().unwrap();
    let path = format!("/tracks/track-1/devices/{device_id}/params/attack");
    run(
        &mut session,
        "param.set",
        json!({ "path": path, "value": 0.2 }),
    );

    let duplicate = session
        .execute(&CommandRequest {
            cmd: "device.add".to_string(),
            args: json!({ "track_id": "track-1", "kind": "amp.adsr" }),
        })
        .unwrap_err();
    assert_eq!(duplicate.code, "invalid_args");

    let state = run(&mut session, "project.state", json!({}));
    let device = &state["project"]["tracks"][0]["devices"][0];
    assert_eq!(device["kind"], "amp.adsr");
    assert!((device["params"]["attack"].as_f64().unwrap() - 0.2).abs() < 1e-9);
    assert!(device["params"]["release"].as_f64().unwrap() > 0.0);

    let error = session
        .execute(&CommandRequest {
            cmd: "param.set".to_string(),
            args: json!({
                "path": format!("/tracks/track-1/devices/{device_id}/params/sustain"),
                "value": 1.5
            }),
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid_args");

    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert!(
        (state["project"]["tracks"][0]["devices"][0]["params"]["attack"]
            .as_f64()
            .unwrap()
            - 0.01)
            .abs()
            < 1e-9
    );
}

#[test]
fn node_commands_add_move_and_remove() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));

    let adsr = run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "amp.adsr" }),
    );
    let gain = run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "gain" }),
    );
    assert_eq!(adsr["node_id"], "device-1");
    assert_eq!(gain["node_id"], "device-2");

    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["devices"][0]["kind"],
        "amp.adsr"
    );
    assert_eq!(state["project"]["tracks"][0]["devices"][1]["kind"], "gain");

    run(
        &mut session,
        "node.move",
        json!({
            "path": "/tracks/track-1/nodes/device-2",
            "before": "/tracks/track-1/nodes/device-1"
        }),
    );
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(state["project"]["tracks"][0]["devices"][0]["kind"], "gain");
    assert_eq!(
        state["project"]["tracks"][0]["devices"][1]["kind"],
        "amp.adsr"
    );
    assert_eq!(
        state["project"]["tracks"][0]["graph"]["connections"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    run(
        &mut session,
        "node.remove",
        json!({ "path": "/tracks/track-1/nodes/device-2" }),
    );
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["devices"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["devices"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn lowpass_device_accepts_cutoff_and_resonance() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    let device = run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "filter.lowpass" }),
    );
    let device_id = device["device_id"].as_str().unwrap();
    run(
        &mut session,
        "param.set",
        json!({
            "path": format!("/tracks/track-1/devices/{device_id}/params/cutoff"),
            "value": 640.0
        }),
    );
    let state = run(&mut session, "project.state", json!({}));
    let device = &state["project"]["tracks"][0]["devices"][0];
    assert_eq!(device["kind"], "filter.lowpass");
    assert!((device["params"]["cutoff"].as_f64().unwrap() - 640.0).abs() < 1e-9);
    assert!((device["params"]["resonance"].as_f64().unwrap() - 0.2).abs() < 1e-9);

    let error = session
        .execute(&CommandRequest {
            cmd: "param.set".to_string(),
            args: json!({
                "path": format!("/tracks/track-1/devices/{device_id}/params/cutoff"),
                "value": 30_000.0
            }),
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid_args");
}

#[test]
fn node_positions_and_batch_remove() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    let filter = run(
        &mut session,
        "node.add",
        json!({ "track_id": "track-1", "kind": "filter.lowpass" }),
    );
    let filter_id = filter["node_id"].as_str().unwrap();
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["graph"]["connections"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    run(
        &mut session,
        "node.position",
        json!({
            "positions": [{
                "path": format!("/tracks/track-1/nodes/{filter_id}"),
                "x": 320.0,
                "y": 140.0
            }]
        }),
    );
    let state = run(&mut session, "project.state", json!({}));
    let node = &state["project"]["tracks"][0]["graph"]["nodes"][filter_id];
    assert_eq!(node["position"], json!([320.0, 140.0]));
    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert!(
        state["project"]["tracks"][0]["graph"]["nodes"][filter_id]
            .get("position")
            .is_none()
    );

    run(
        &mut session,
        "node.remove_many",
        json!({
            "paths": [format!("/tracks/track-1/nodes/{filter_id}")]
        }),
    );
    let state = run(&mut session, "project.state", json!({}));
    assert!(
        state["project"]["tracks"][0]["graph"]["nodes"]
            .get(filter_id)
            .is_none()
    );
}

#[test]
fn duplicate_nodes_copies_params_and_position() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    let filter = run(
        &mut session,
        "node.add",
        json!({
            "track_id": "track-1",
            "kind": "filter.lowpass",
            "params": { "cutoff": 640.0 },
            "position": [100.0, 80.0]
        }),
    );
    let filter_id = filter["node_id"].as_str().unwrap();
    let duplicated = run(
        &mut session,
        "node.duplicate",
        json!({
            "paths": [format!("/tracks/track-1/nodes/{filter_id}")],
            "offset": [30.0, 20.0]
        }),
    );
    let new_id = duplicated["nodes"][0]["node_id"].as_str().unwrap();
    assert_ne!(new_id, filter_id);
    let state = run(&mut session, "project.state", json!({}));
    let node = &state["project"]["tracks"][0]["graph"]["nodes"][new_id];
    assert_eq!(node["type_id"], "audio.filter.lowpass");
    assert_eq!(node["params"]["cutoff"], 640.0);
    assert_eq!(node["position"], json!([130.0, 100.0]));
}

#[test]
fn node_types_expose_audio_registry() {
    let mut session = Session::default();
    let types = run(&mut session, "node.types", json!({}));
    assert!(
        types["audio_types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node_type| node_type["id"] == "audio.filter.highpass")
    );
    assert!(
        types["types"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node_type| node_type["id"] == "math.add")
    );
}

#[test]
fn highpass_and_drive_render() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(
        &mut session,
        "track.create",
        json!({ "name": "lead", "source": "sine" }),
    );
    run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "filter.highpass" }),
    );
    run(
        &mut session,
        "device.add",
        json!({
            "track_id": "track-1",
            "kind": "drive",
            "params": { "drive": 2.0 }
        }),
    );
    let path = temp_path("highpass-drive");
    let rendered = run(
        &mut session,
        "render.segment",
        json!({ "duration_seconds": 0.5, "path": path }),
    );
    assert!(rendered["rms"].as_f64().unwrap() > 0.001);
    let _ = std::fs::remove_file(path);
}

#[test]
fn branch_graph_connects_mix_and_renders() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({}));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    run(
        &mut session,
        "clip.create",
        json!({ "track_id": "track-1", "start": 0.0, "duration": 1.0 }),
    );
    run(
        &mut session,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "pitch": 60,
            "start": 0.0,
            "duration": 0.5,
            "velocity": 1.0
        }),
    );
    run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "filter.lowpass" }),
    );
    let mix = run(
        &mut session,
        "device.add",
        json!({ "track_id": "track-1", "kind": "mix" }),
    );
    let mix_id = mix["node_id"].as_str().unwrap().to_string();
    run(
        &mut session,
        "node.connect",
        json!({
            "from": "/tracks/track-1/nodes/instrument",
            "to": format!("/tracks/track-1/nodes/{mix_id}"),
            "to_port": "in2"
        }),
    );
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["graph"]["connections"]
            .as_array()
            .unwrap()
            .len(),
        4
    );

    let branch = temp_path("branch");
    let out = run(
        &mut session,
        "render.segment",
        json!({ "duration_seconds": 0.4, "path": branch }),
    );
    assert!(out["peak"].as_f64().unwrap() > 0.0);

    run(
        &mut session,
        "node.disconnect",
        json!({
            "from": "/tracks/track-1/nodes/instrument",
            "to": format!("/tracks/track-1/nodes/{mix_id}"),
            "to_port": "in2"
        }),
    );
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["graph"]["connections"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    run(&mut session, "undo", json!({}));
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["tracks"][0]["graph"]["connections"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[test]
fn tempo_change_keeps_musical_positions() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "tempo" }));
    run(&mut session, "track.create", json!({ "name": "lead" }));
    run(
        &mut session,
        "clip.create",
        json!({ "track_id": "track-1", "start": 0.0, "duration": 2.0 }),
    );
    run(
        &mut session,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "pitch": 60,
            "start": 1.0,
            "duration": 0.5,
        }),
    );

    let before = run(&mut session, "project.state", json!({}));
    assert_eq!(before["project"]["tempo_bpm"], 120.0);
    assert!(
        before["project"]["tracks"][0]["clips"][0]["start"]
            .as_f64()
            .unwrap()
            .abs()
            < 1e-3
    );
    assert!(
        (before["project"]["tracks"][0]["clips"][0]["notes"][0]["start"]
            .as_f64()
            .unwrap()
            - 1.0)
            .abs()
            < 1e-3
    );

    let updated = run(
        &mut session,
        "project.update",
        json!({ "tempo_bpm": 60.0, "time_signature": { "numerator": 3, "denominator": 4 }, "length_bars": 2 }),
    );
    assert_eq!(updated["project"]["tempo_bpm"], 60.0);
    assert_eq!(updated["project"]["length_bars"], 2);

    let after = run(&mut session, "project.state", json!({}));
    let note = &after["project"]["tracks"][0]["clips"][0]["notes"][0];
    assert!((note["start"].as_f64().unwrap() - 2.0).abs() < 1e-3);
    assert!((after["project"]["loop_end"].as_f64().unwrap() - 6.0).abs() < 1e-3);

    run(&mut session, "undo", json!({}));
    let restored = run(&mut session, "project.state", json!({}));
    assert_eq!(restored["project"]["tempo_bpm"], 120.0);
    assert!(
        (restored["project"]["tracks"][0]["clips"][0]["notes"][0]["start"]
            .as_f64()
            .unwrap()
            - 1.0)
            .abs()
            < 1e-3
    );
}

#[test]
fn project_open_rejects_v3_format_version() {
    let path = project_temp_path("bad-version");
    std::fs::write(
        &path,
        r#"{"format_version":3,"project":{"name":"x","sample_rate":48000,"tracks":[]}}"#,
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

#[test]
fn master_plugin_is_saved_and_restored_with_project() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "plugin" }));
    run(
        &mut session,
        "project.master_plugin",
        json!({
            "path": "D:/Plugins/effect.clap",
            "id": "com.example.effect",
            "state": "AAECAwQ="
        }),
    );
    let state = run(&mut session, "project.state", json!({}));
    assert_eq!(
        state["project"]["master_plugin"]["id"],
        "com.example.effect"
    );
    assert_eq!(state["project"]["master_plugin"]["state"], "AAECAwQ=");

    let path = project_temp_path("master-plugin");
    run(&mut session, "project.save", json!({ "path": path }));
    run(&mut session, "project.create", json!({ "name": "empty" }));
    run(&mut session, "project.open", json!({ "path": path }));
    let reopened = run(&mut session, "project.state", json!({}));
    assert_eq!(
        reopened["project"]["master_plugin"]["path"],
        "D:/Plugins/effect.clap"
    );
    let _ = std::fs::remove_file(path);

    run(&mut session, "project.master_plugin_off", json!({}));
    let cleared = run(&mut session, "project.state", json!({}));
    assert!(cleared["project"]["master_plugin"].is_null());
}

#[test]
fn plugin_node_reaches_engine_plan() {
    let mut session = Session::default();
    run(
        &mut session,
        "project.create",
        json!({ "name": "plugin-node" }),
    );
    let track = run(
        &mut session,
        "track.create",
        json!({ "name": "lead", "source": "sine" }),
    );
    let track_id = track["track_id"].as_str().unwrap().to_string();
    run(
        &mut session,
        "device.add",
        json!({
            "track_id": track_id,
            "kind": "plugin",
            "plugin_path": "D:/Plugins/effect.clap",
            "plugin_id": "com.example.effect"
        }),
    );
    let project = run(&mut session, "project.state", json!({}));
    let nodes = &project["project"]["tracks"][0]["graph"]["nodes"];
    assert!(nodes.as_object().unwrap().values().any(|node| {
        node["type_id"] == "audio.plugin" && node["plugin"]["id"] == "com.example.effect"
    }));
    let project = session.project().unwrap().clone();
    let track = &project.tracks[0];
    let (_, _, plan) = track.playback_graph(&project.node_groups).unwrap();
    assert_eq!(plan.plugin.as_ref().unwrap().id, "com.example.effect");
}

#[test]
fn group_commands_create_rename_and_delete() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "groups" }));
    let created = run(
        &mut session,
        "group.create",
        json!({ "name": "Aux Voice", "group_id": "aux.voice" }),
    );
    assert_eq!(created["group_id"], "aux.voice");
    run(
        &mut session,
        "group.rename",
        json!({ "group_id": "aux.voice", "name": "Aux" }),
    );
    assert_eq!(
        session.project().unwrap().node_groups["aux.voice"].name,
        "Aux"
    );
    run(
        &mut session,
        "group.delete",
        json!({ "group_id": "aux.voice" }),
    );
    assert!(
        !session
            .project()
            .unwrap()
            .node_groups
            .contains_key("aux.voice")
    );
}

#[test]
fn piano_tracks_share_one_node_group() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "groups" }));
    for name in ["a", "b"] {
        run(
            &mut session,
            "track.create",
            json!({ "name": name, "source": "piano" }),
        );
    }
    let project = session.project().unwrap().clone();
    assert_eq!(project.node_groups.len(), 1);
    let group_ids: Vec<String> = project
        .tracks
        .iter()
        .map(|track| {
            track
                .graph
                .nodes()
                .find(|node| node.type_id == "audio.group")
                .and_then(|node| node.group().map(str::to_string))
                .unwrap()
        })
        .collect();
    assert_eq!(group_ids, ["piano.voice", "piano.voice"]);
    let voice = serde_json::to_value(&project.node_groups["piano.voice"].graph).unwrap();
    run(
        &mut session,
        "group.voice",
        json!({ "group_id": "piano.voice", "voice": voice }),
    );
    assert_eq!(session.project().unwrap().node_groups.len(), 1);
}

#[test]
fn math_nodes_flow_into_voice_engine() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "math" }));
    run(
        &mut session,
        "track.create",
        json!({ "name": "piano", "source": "piano" }),
    );
    run(
        &mut session,
        "clip.create",
        json!({ "track_id": "track-1", "start": 0.0, "duration": 1.0 }),
    );
    run(
        &mut session,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "pitch": 60,
            "start": 0.0,
            "duration": 0.5,
            "velocity": 0.8
        }),
    );
    let project = session.project().unwrap().clone();
    let mut voice = serde_json::to_value(&project.node_groups["piano.voice"].graph).unwrap();
    let connections = voice["connections"].as_array_mut().unwrap();
    connections.retain(|connection| {
        !(connection["to"]["node"] == "osc" && connection["to"]["port"] == "pitch")
    });
    connections.push(json!({
        "from": { "node": "voice-in", "port": "pitch" },
        "to": { "node": "add", "port": "a" }
    }));
    connections.push(json!({
        "from": { "node": "offset", "port": "out" },
        "to": { "node": "add", "port": "b" }
    }));
    connections.push(json!({
        "from": { "node": "add", "port": "out" },
        "to": { "node": "osc", "port": "pitch" }
    }));
    voice["nodes"]["add"] = json!({
        "id": "add",
        "type_id": "math.add",
        "params": {}
    });
    voice["nodes"]["offset"] = json!({
        "id": "offset",
        "type_id": "value.float",
        "params": { "value": 12.0 }
    });
    run(
        &mut session,
        "group.voice",
        json!({ "group_id": "piano.voice", "voice": voice }),
    );
    let path = temp_path("math");
    let rendered = run(
        &mut session,
        "render.segment",
        json!({ "path": path, "duration_seconds": 0.5 }),
    );
    assert!(rendered["rms"].as_f64().unwrap() > 0.001);
    let _ = std::fs::remove_file(path);
}

#[test]
fn nested_group_compiles_into_voice_plan() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "nested" }));
    run(
        &mut session,
        "track.create",
        json!({ "name": "piano", "source": "piano" }),
    );
    run(
        &mut session,
        "group.create",
        json!({ "name": "Aux", "group_id": "aux.voice" }),
    );
    let project = session.project().unwrap().clone();
    let mut voice = serde_json::to_value(&project.node_groups["piano.voice"].graph).unwrap();
    let connections = voice["connections"].as_array_mut().unwrap();
    connections.retain(|connection| connection["to"]["port"] != "audio");
    connections.push(json!({
        "from": { "node": "voice-in", "port": "pitch" },
        "to": { "node": "nested", "port": "pitch" }
    }));
    connections.push(json!({
        "from": { "node": "nested", "port": "audio" },
        "to": { "node": "out", "port": "audio" }
    }));
    voice["nodes"]["nested"] = json!({
        "id": "nested",
        "type_id": "voice.group",
        "params": {},
        "group": "aux.voice"
    });
    run(
        &mut session,
        "group.voice",
        json!({ "group_id": "piano.voice", "voice": voice }),
    );
    let project = session.project().unwrap().clone();
    let plan = project.tracks[0]
        .playback_graph(&project.node_groups)
        .unwrap()
        .2;
    assert!(plan.piano.unwrap().voice.unwrap().nodes.len() > 6);
}

#[test]
fn piano_track_renders_audible_wav() {
    let mut session = Session::default();
    run(&mut session, "project.create", json!({ "name": "piano" }));
    run(
        &mut session,
        "track.create",
        json!({ "name": "piano", "source": "piano" }),
    );
    run(
        &mut session,
        "clip.create",
        json!({ "track_id": "track-1", "start": 0.0, "duration": 2.0 }),
    );
    run(
        &mut session,
        "note.add",
        json!({
            "path": "/tracks/track-1/clips/clip-1",
            "pitch": 60,
            "start": 0.0,
            "duration": 1.0,
            "velocity": 0.8,
        }),
    );
    let project = session.project().unwrap().clone();
    let plan = project.tracks[0]
        .playback_graph(&project.node_groups)
        .unwrap()
        .2;
    assert!(plan.piano.as_ref().unwrap().voice.is_some());
    let path = temp_path("piano");
    let rendered = run(
        &mut session,
        "render.segment",
        json!({ "path": path, "duration_seconds": 1.0 }),
    );
    let rms = rendered["rms"].as_f64().unwrap();
    assert!(rms > 0.001, "钢琴轨渲染没有声音：{rms}");
    let _ = std::fs::remove_file(path);
}
