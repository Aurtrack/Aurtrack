export type CommandResponse<T> = {
  ok: boolean;
  data: T;
  warnings: string[];
  undo_token?: string;
};

export type CommandError = {
  code: string;
  message: string;
};

export type Device = {
  id: string;
  kind: string;
  params: Record<string, number>;
};

export type Note = {
  id: string;
  pitch: number;
  start: number;
  duration: number;
  start_tick: number;
  duration_tick: number;
  velocity: number;
};

export type Clip = {
  id: string;
  name: string;
  start: number;
  duration: number;
  start_tick: number;
  duration_tick: number;
  notes: Note[];
};

export type TimeSignature = {
  numerator: number;
  denominator: number;
};

export type Source =
  | { kind: "silence" }
  | { kind: "sine"; frequency: number }
  | { kind: "piano" };

export type GraphNode = {
  id: string;
  kind: string;
  type_id?: string;
  params: Record<string, number>;
  position?: [number, number];
  plugin?: { path: string; id: string; state?: string };
  group?: string;
  frame?: string;
  muted?: boolean;
  collapsed?: boolean;
};

export type VoiceNode = {
  id: string;
  type_id: string;
  params: Record<string, number>;
  position?: [number, number];
  group?: string;
  frame?: string;
  muted?: boolean;
  collapsed?: boolean;
};

export type VoiceFrame = {
  id: string;
  name: string;
  position: [number, number];
  size: [number, number];
};

export type VoiceConnection = {
  from: { node: string; port: string };
  to: { node: string; port: string };
};

export type VoiceGraph = {
  nodes: Record<string, VoiceNode>;
  connections: VoiceConnection[];
  frames?: Record<string, VoiceFrame>;
};

export type NodeGroupSocket = {
  id: string;
  name: string;
  type: "float" | "gate" | "audio";
  default?: number;
  minimum?: number;
  maximum?: number;
  step?: number;
  panel?: string;
  description?: string;
};

export type NodeGroupInterface = {
  inputs: NodeGroupSocket[];
  outputs: NodeGroupSocket[];
};

export type NodeTypeSocket = {
  id: string;
  name: string;
  type: "float" | "gate" | "audio";
  default?: number;
  panel?: string;
};

export type NodeTypeParam = {
  name: string;
  default: number;
  minimum: number;
  maximum: number;
  step?: number;
  panel?: string;
};

export type NodeTypeInfo = {
  id: string;
  name: string;
  category: string;
  multi_input?: boolean;
  inputs: NodeTypeSocket[];
  outputs: NodeTypeSocket[];
  params: NodeTypeParam[];
};

export type NodeGroup = {
  id: string;
  name: string;
  interface: NodeGroupInterface;
  graph: VoiceGraph;
};

export type AudioGroup = {
  id: string;
  name: string;
  interface: NodeGroupInterface;
  graph: TrackGraph;
};

export type GraphConnection = {
  from: { node: string; port: string };
  to: { node: string; port: string };
};

export type TrackGraph = {
  nodes: Record<string, GraphNode>;
  connections: GraphConnection[];
  frames?: Record<string, VoiceFrame>;
};

export type Track = {
  id: string;
  name: string;
  source: Source;
  devices: Device[];
  graph?: TrackGraph;
  clips: Clip[];
};

export type Project = {
  name: string;
  sample_rate: number;
  tempo_bpm: number;
  time_signature: TimeSignature;
  length_bars: number;
  length_seconds: number;
  bar_seconds: number;
  loop_start: number;
  loop_end: number;
  loop_start_tick: number;
  loop_end_tick: number;
  tracks: Track[];
  node_groups: Record<string, NodeGroup>;
  audio_groups?: Record<string, AudioGroup>;
};

export type ProjectState = {
  project: Project | null;
};

type Invoke = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

type CloseRequestedEvent = {
  preventDefault: () => void;
};

type TauriWindow = {
  onCloseRequested: (
    handler: (event: CloseRequestedEvent) => void | Promise<void>,
  ) => Promise<() => void>;
  close: () => Promise<void>;
  destroy: () => Promise<void>;
};

declare global {
  interface Window {
    __TAURI__?: {
      core: {
        invoke: Invoke;
      };
      window?: {
        getCurrentWindow: () => TauriWindow;
      };
    };
  }
}

function getInvoke(): Invoke {
  const tauri = window.__TAURI__;
  if (!tauri) {
    throw new Error("Tauri API 不可用，窗口没有正常加载");
  }
  return tauri.core.invoke;
}

export async function call<T>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  const response = await getInvoke()<CommandResponse<T>>("execute", {
    request: { cmd, args },
  });
  if (!response || response.ok !== true) {
    throw new Error("命令执行失败");
  }
  return response.data;
}

export async function renderPath(): Promise<string> {
  return getInvoke()<string>("render_path");
}

export async function openFile(path: string): Promise<void> {
  await getInvoke()<void>("open_file", { path });
}

export type TransportState = {
  playing: boolean;
  position: number;
  sample_rate: number;
  metronome: boolean;
};

export type AudioInfo = {
  available: boolean;
  sample_rate: number;
  device: string | null;
};

export async function transportPlay(): Promise<void> {
  await call("transport.play");
}

export async function transportStop(): Promise<void> {
  await call("transport.stop");
}

export async function transportSeek(seconds: number): Promise<void> {
  await call("transport.seek", { seconds });
}

export async function transportState(): Promise<TransportState> {
  return call<TransportState>("transport.state");
}

export async function setMasterGain(gain: number): Promise<void> {
  await call("transport.gain", { gain });
}

export async function audioInfo(): Promise<AudioInfo> {
  return getInvoke()<AudioInfo>("audio_info");
}

export async function pickProjectPath(
  mode: "save" | "open",
): Promise<string | null> {
  return getInvoke()<string | null>("pick_project_path", { mode });
}

export async function recoveryPath(): Promise<string> {
  return getInvoke()<string>("recovery_path");
}

export async function recoveryExists(): Promise<boolean> {
  return getInvoke()<boolean>("recovery_exists");
}

export async function removeRecoveryFile(): Promise<void> {
  await getInvoke()<void>("recovery_remove");
}

export async function onCloseRequested(
  handler: (event: CloseRequestedEvent) => void | Promise<void>,
): Promise<() => void> {
  const tauri = window.__TAURI__;
  if (!tauri?.window) {
    throw new Error("Tauri 窗口 API 不可用");
  }
  return tauri.window.getCurrentWindow().onCloseRequested(handler);
}

export async function requestWindowClose(): Promise<void> {
  const tauri = window.__TAURI__;
  if (!tauri?.window) {
    throw new Error("Tauri 窗口 API 不可用");
  }
  await tauri.window.getCurrentWindow().close();
}

export async function destroyWindow(): Promise<void> {
  const tauri = window.__TAURI__;
  if (!tauri?.window) {
    throw new Error("Tauri 窗口 API 不可用");
  }
  await tauri.window.getCurrentWindow().destroy();
}

export async function projectState(): Promise<ProjectState> {
  return call<ProjectState>("project.state");
}

export async function undoCommand(steps = 1): Promise<void> {
  await call("undo", { steps });
}

export async function redoCommand(steps = 1): Promise<void> {
  await call("redo", { steps });
}

export async function removeTrack(trackId: string): Promise<void> {
  await call("track.remove", { track_id: trackId });
}

export async function removeDevice(path: string): Promise<void> {
  await call("device.remove", { path });
}

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (typeof error === "object" && error !== null && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}
