import type { Message } from "./types";
import type { MessageKey } from "./zh-CN";

export const en: Record<MessageKey, Message> = {
  "app.tagline": "FLAT SCHEMATIC / AUDIO WORKSTATION",

  "header.audio": (params) => `AUDIO ${params.rate} HZ`,
  "header.noAudio": "NO AUDIO",
  "header.audioUnknown": "AUDIO ?",
  "header.build": (params) => `BUILD ${params.version}`,
  "header.preAlpha": "PRE-ALPHA",
  "header.switchLanguage": "Switch language",

  "timeline.title": "TIMELINE",
  "timeline.meta": (params) => {
    const count = Number(params.tracks);
    return `${count} TRACK${count === 1 ? "" : "S"} / ${params.duration} S`;
  },
  "timeline.empty": "NO TRACKS",
  "timeline.hint": "CLICK TO SEEK / DRAG TO EDIT",
  "timeline.playhead": (params) => `T+${params.seconds} S`,
  "timeline.trackMeta": (params) =>
    `SINE ${params.frequency} HZ / ${params.devices} DEV`,
  "timeline.silenceMeta": (params) => `SILENCE / ${params.devices} DEV`,
  "timeline.clipMeta": (params) =>
    `${params.clips} CLIPS / ${params.notes} NOTES`,

  "clips.title": "CLIPS",
  "clips.total": (params) => `${params.count} TOTAL`,
  "clips.noTrack": "NO TRACK SELECTED",
  "clips.namePlaceholder": "CLIP NAME",
  "clips.start": "START",
  "clips.duration": "DURATION",
  "clips.add": "ADD CLIP",
  "clips.empty": "NO CLIPS",
  "clips.removeTitle": "Remove clip",
  "clips.defaultName": (params) => `Clip ${params.index}`,
  "clips.range": (params) =>
    `${params.start}–${params.end} s · ${params.notes} NOTES`,

  "notes.title": "NOTES",
  "notes.total": (params) => `${params.count} TOTAL`,
  "notes.empty": "NO NOTES",
  "notes.add": "ADD NOTE",
  "notes.pitch": "PITCH",
  "notes.start": "START",
  "notes.duration": "DURATION",
  "notes.velocity": "VEL",
  "notes.removeTitle": "Remove note",
  "notes.meta": (params) =>
    `${params.start}+${params.duration} s · ${params.velocity}%`,

  "piano.title": "PIANO ROLL",
  "piano.empty": "SELECT A CLIP",
  "piano.meta": (params) => `${params.clip} · ${params.notes} NOTES`,
  "piano.hint": "CLICK TO ADD · DRAG TO MOVE · EDGE TO RESIZE · RIGHT-CLICK TO DELETE",

  "tracks.title": "TRACKS",
  "tracks.total": (params) => `${params.count} TOTAL`,
  "tracks.namePlaceholder": "TRACK NAME",
  "tracks.source.sine": "SINE",
  "tracks.source.silence": "SILENCE",
  "tracks.add": "ADD",
  "tracks.empty": "NO TRACKS",
  "tracks.removeTitle": "Remove track",
  "tracks.defaultName": (params) => `Track ${params.index}`,
  "tracks.meta.sine": (params) =>
    `SINE ${params.frequency} HZ · ${params.devices} DEV`,
  "tracks.meta.silence": (params) => `SILENCE · ${params.devices} DEV`,

  "inspector.title": "INSPECTOR",
  "inspector.none": "NONE",
  "inspector.empty": "NO TRACK SELECTED",
  "inspector.addGain": "+ GAIN",
  "inspector.removeDeviceTitle": "Remove device",
  "inspector.source.sine": (params) => `SINE ${params.frequency} HZ`,
  "inspector.source.silence": "SILENCE",

  "device.kind.gain": "GAIN",

  "transport.title": "TRANSPORT",
  "transport.commands": (params) => `${params.count} COMMANDS`,
  "transport.master": "MASTER",
  "transport.play": "PLAY",
  "transport.stop": "STOP",
  "transport.render": "RENDER WAV",
  "transport.undo": "UNDO",
  "transport.redo": "REDO",
  "transport.save": "SAVE",
  "transport.open": "OPEN",
  "transport.file": (params) => `FILE: ${params.file}`,
  "transport.unsaved": "UNSAVED",
  "transport.status": (params) => `STATUS: ${params.status}`,

  "log.title": "COMMAND LOG",
  "log.entries": (params) => `${params.count} ENTRIES`,

  "project.defaultName": "Aurtrack Project",

  "status.connecting": "Connecting to session…",
  "status.ready": "Session ready. Add a track, or press PLAY.",
  "status.initFailed": (params) => `Init failed: ${params.message}`,
  "status.trackCreated": (params) => `Created ${params.id}`,
  "status.trackCreateFailed": (params) =>
    `Create track failed: ${params.message}`,
  "status.trackRemoved": (params) => `Removed ${params.id}`,
  "status.trackRemoveFailed": (params) =>
    `Remove track failed: ${params.message}`,
  "status.gainAdded": "Gain added",
  "status.gainAddFailed": (params) => `Add device failed: ${params.message}`,
  "status.deviceRemoved": "Device removed",
  "status.deviceRemoveFailed": (params) =>
    `Remove device failed: ${params.message}`,
  "status.gainSetFailed": (params) => `Set gain failed: ${params.message}`,
  "status.undone": "Undone",
  "status.undoFailed": (params) => `Undo failed: ${params.message}`,
  "status.redone": "Redone",
  "status.redoFailed": (params) => `Redo failed: ${params.message}`,
  "status.playing": "PLAYING",
  "status.playFailed": (params) => `Play failed: ${params.message}`,
  "status.stopped": "STOPPED",
  "status.stopFailed": (params) => `Stop failed: ${params.message}`,
  "status.rendering": "Rendering WAV…",
  "status.rendered": (params) =>
    `Rendered: peak ${params.peak} · rms ${params.rms} · ${params.frames} frames`,
  "status.renderFailed": (params) => `Render failed: ${params.message}`,
  "status.saved": (params) => `Saved: ${params.file}`,
  "status.saveFailed": (params) => `Save failed: ${params.message}`,
  "status.opened": (params) => `Opened: ${params.file}`,
  "status.openFailed": (params) => `Open failed: ${params.message}`,
  "status.clipCreated": (params) => `Created clip ${params.id}`,
  "status.clipCreateFailed": (params) =>
    `Create clip failed: ${params.message}`,
  "status.clipRemoved": (params) => `Removed clip ${params.id}`,
  "status.clipRemoveFailed": (params) =>
    `Remove clip failed: ${params.message}`,
  "status.clipUpdated": (params) => `Updated clip ${params.id}`,
  "status.clipUpdateFailed": (params) =>
    `Update clip failed: ${params.message}`,
  "status.noteAdded": (params) => `Added note ${params.id}`,
  "status.noteAddFailed": (params) => `Add note failed: ${params.message}`,
  "status.noteRemoved": (params) => `Removed note ${params.id}`,
  "status.noteRemoveFailed": (params) =>
    `Remove note failed: ${params.message}`,
  "status.noteUpdated": (params) => `Updated note ${params.id}`,
  "status.noteUpdateFailed": (params) =>
    `Update note failed: ${params.message}`,

  "error.not_found": "Not found",
  "error.invalid_args": "Invalid arguments",
  "error.ipc_error": "Session connection error",
  "error.nothing_to_undo": "Nothing to undo",
  "error.nothing_to_redo": "Nothing to redo",
  "error.audio_unavailable": "No audio device available",
  "error.unknown_command": "Unknown command",
  "error.internal_error": "Internal error",
  "error.io_error": "File I/O failed",
};
