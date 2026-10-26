/** Blender 风格节点卡片的共享尺寸。 */
export const NODE_WIDTH = 204;
export const NODE_HEADER = 26;
export const NODE_ROW = 22;
export const NODE_BODY_TOP = 6;
export const NODE_BOTTOM = 7;

export function expandedNodeHeight(rows: number, groupHeader: number): number {
  return (
    NODE_HEADER +
    groupHeader +
    NODE_BODY_TOP +
    rows * NODE_ROW +
    NODE_BOTTOM
  );
}

export function expandedPortY(index: number, groupHeader: number): number {
  return (
    NODE_HEADER +
    groupHeader +
    NODE_BODY_TOP +
    index * NODE_ROW +
    NODE_ROW / 2
  );
}

export function collapsedNodeHeight(rows: number): number {
  return NODE_HEADER + 4 + Math.max(1, rows) * 6;
}

export function collapsedPortY(index: number): number {
  return NODE_HEADER + 3 + index * 6;
}
