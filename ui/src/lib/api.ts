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

type Invoke = <T>(
  command: string,
  args?: Record<string, unknown>,
) => Promise<T>;

declare global {
  interface Window {
    __TAURI__?: {
      core: {
        invoke: Invoke;
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
};

export type AudioInfo = {
  available: boolean;
  sample_rate: number;
  device: string | null;
};

export async function transportPlay(): Promise<void> {
  await getInvoke()<void>("transport_play");
}

export async function transportStop(): Promise<void> {
  await getInvoke()<void>("transport_stop");
}

export async function transportSeek(seconds: number): Promise<void> {
  await getInvoke()<void>("transport_seek", { seconds });
}

export async function transportState(): Promise<TransportState> {
  return getInvoke()<TransportState>("transport_state");
}

export async function setMasterGain(gain: number): Promise<void> {
  await getInvoke()<void>("set_master_gain", { gain });
}

export async function audioInfo(): Promise<AudioInfo> {
  return getInvoke()<AudioInfo>("audio_info");
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
