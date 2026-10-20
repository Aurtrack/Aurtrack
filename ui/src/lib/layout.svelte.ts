import type { MessageKey } from "./i18n.svelte";

export const PANEL_KINDS = [
  "timeline",
  "piano",
  "tracks",
  "clips",
  "inspector",
  "transport",
  "log",
] as const;

export type PanelKind = (typeof PANEL_KINDS)[number];
export type SplitDirection = "row" | "column";

export const PANEL_LABEL_KEYS: Record<PanelKind, MessageKey> = {
  timeline: "panel.timeline",
  piano: "panel.piano",
  tracks: "panel.tracks",
  clips: "panel.clips",
  inspector: "panel.inspector",
  transport: "panel.transport",
  log: "panel.log",
};

export type PaneNode = {
  kind: "pane";
  id: string;
  panel: PanelKind;
};

export type SplitNode = {
  kind: "split";
  id: string;
  direction: SplitDirection;
  ratio: number;
  first: LayoutNode;
  second: LayoutNode;
};

export type LayoutNode = PaneNode | SplitNode;

const STORAGE_KEY = "aurtrack.layout.v1";
const MIN_RATIO = 0.15;
const MAX_RATIO = 0.85;

let sequence = 0;

function createId(prefix: string): string {
  sequence += 1;
  return `${prefix}-${Date.now().toString(36)}-${sequence}`;
}

function makeLeaf(panel: PanelKind): PaneNode {
  return { kind: "pane", id: createId("pane"), panel };
}

function makeSplit(
  direction: SplitDirection,
  ratio: number,
  first: LayoutNode,
  second: LayoutNode,
): SplitNode {
  return {
    kind: "split",
    id: createId("split"),
    direction,
    ratio,
    first,
    second,
  };
}

function defaultLayout(): LayoutNode {
  const timeline = makeLeaf("timeline");
  const piano = makeLeaf("piano");
  const log = makeLeaf("log");
  const tracks = makeLeaf("tracks");
  const clips = makeLeaf("clips");
  const inspector = makeLeaf("inspector");
  const transport = makeLeaf("transport");

  const left = makeSplit(
    "column",
    0.34,
    timeline,
    makeSplit("column", 0.76, piano, log),
  );
  const right = makeSplit(
    "column",
    0.27,
    tracks,
    makeSplit(
      "column",
      0.49,
      clips,
      makeSplit("column", 0.52, inspector, transport),
    ),
  );

  return makeSplit("row", 0.62, left, right);
}

function parseLayout(value: unknown): LayoutNode | null {
  if (typeof value !== "object" || value === null) return null;
  const raw = value as Record<string, unknown>;

  if (raw.kind === "pane") {
    if (
      typeof raw.id !== "string" ||
      typeof raw.panel !== "string" ||
      !PANEL_KINDS.includes(raw.panel as PanelKind)
    ) {
      return null;
    }
    return {
      kind: "pane",
      id: raw.id,
      panel: raw.panel as PanelKind,
    };
  }

  if (raw.kind === "split") {
    if (
      typeof raw.id !== "string" ||
      (raw.direction !== "row" && raw.direction !== "column") ||
      typeof raw.ratio !== "number" ||
      !Number.isFinite(raw.ratio)
    ) {
      return null;
    }
    const first = parseLayout(raw.first);
    const second = parseLayout(raw.second);
    if (!first || !second) return null;
    return {
      kind: "split",
      id: raw.id,
      direction: raw.direction,
      ratio: Math.min(MAX_RATIO, Math.max(MIN_RATIO, raw.ratio)),
      first,
      second,
    };
  }

  return null;
}

function loadLayout(): LayoutNode | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    return parseLayout(JSON.parse(raw));
  } catch {
    return null;
  }
}

export class LayoutStore {
  root = $state<LayoutNode>(defaultLayout());
  leafCount = $derived(this.countLeaves(this.root));
  canClose = $derived(this.leafCount > 1);
  private saveTimer: number | null = null;

  constructor() {
    const stored = loadLayout();
    if (stored) this.root = stored;
  }

  setPanel(id: string, panel: PanelKind) {
    this.root = this.mapLeaf(this.root, id, (leaf) => ({ ...leaf, panel }));
    this.persist();
  }

  split(id: string, direction: SplitDirection) {
    this.root = this.mapLeaf(this.root, id, (leaf) =>
      makeSplit(direction, 0.5, leaf, makeLeaf(leaf.panel)),
    );
    this.persist();
  }

  close(id: string) {
    if (!this.canClose) return;
    const next = this.removeNode(this.root, id);
    if (next) {
      this.root = next;
      this.persist();
    }
  }

  setRatio(id: string, ratio: number) {
    const clamped = Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio));
    this.root = this.mapSplit(this.root, id, (node) => ({
      ...node,
      ratio: clamped,
    }));
    this.persist();
  }

  reset() {
    this.root = defaultLayout();
    this.persist();
  }

  private countLeaves(node: LayoutNode): number {
    if (node.kind === "pane") return 1;
    return this.countLeaves(node.first) + this.countLeaves(node.second);
  }

  private mapLeaf(
    node: LayoutNode,
    id: string,
    update: (leaf: PaneNode) => LayoutNode,
  ): LayoutNode {
    if (node.kind === "pane") {
      return node.id === id ? update(node) : node;
    }
    const first = this.mapLeaf(node.first, id, update);
    const second = this.mapLeaf(node.second, id, update);
    if (first === node.first && second === node.second) return node;
    return { ...node, first, second };
  }

  private mapSplit(
    node: LayoutNode,
    id: string,
    update: (node: SplitNode) => SplitNode,
  ): LayoutNode {
    if (node.kind === "pane") return node;
    if (node.id === id) return update(node);
    const first = this.mapSplit(node.first, id, update);
    const second = this.mapSplit(node.second, id, update);
    if (first === node.first && second === node.second) return node;
    return { ...node, first, second };
  }

  private removeNode(node: LayoutNode, id: string): LayoutNode | null {
    if (node.kind === "pane") return node.id === id ? null : node;
    const first = this.removeNode(node.first, id);
    if (first === null) return node.second;
    const second = this.removeNode(node.second, id);
    if (second === null) return node.first;
    if (first === node.first && second === node.second) return node;
    return { ...node, first, second };
  }

  private persist() {
    if (this.saveTimer !== null) window.clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = null;
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(this.root));
      } catch {
        // 布局是本地偏好，存储失败不影响当前会话。
      }
    }, 150);
  }
}

export const layout = new LayoutStore();
