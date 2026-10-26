import type { NodeTypeInfo } from "./api";
import { tLoose } from "./i18n.svelte";

/** 节点类型显示名，后端英文名只作兜底。 */
export function nodeTypeLabel(
  nodeType: Pick<NodeTypeInfo, "id" | "name">,
): string {
  return tLoose(`node.${nodeType.id}`, nodeType.name);
}

/** 节点分类显示名。 */
export function nodeCategoryLabel(category: string): string {
  return tLoose(`nodeCategory.${category}`, category);
}

/** 内置 socket 显示名，用户自定义接口名原样保留。 */
export function socketLabel(name: string): string {
  const key = name.trim().toLowerCase().replaceAll(" ", "_");
  return tLoose(`socket.${key}`, name);
}

/** 内置参数显示名。 */
export function paramLabel(name: string): string {
  return tLoose(`param.${name.toLowerCase()}`, name);
}
