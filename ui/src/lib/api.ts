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
  velocity: number;
};

export type Clip = {
  id: string;
  name: string;
  start: number;
  duration: number;
  notes: Note[];
};

export type Source =
  | { kind: "silence" }
  | { kind: "sine"; frequency: number };

export type Track = {
  id: string;
  name: string;
  source: Source;
  devices: Device[];
  clips: Clip[];
};

export type Project = {
  name: string;
  sample_rate: number;
  tracks: Track[];
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
