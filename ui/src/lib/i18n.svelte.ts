import { en } from "./locales/en";
import { zhCN, type MessageKey } from "./locales/zh-CN";
import type { Message } from "./locales/types";

export type Locale = "zh-CN" | "en";
export type { MessageKey };

const dictionaries: Record<Locale, Record<MessageKey, Message>> = {
  "zh-CN": zhCN,
  en,
};

const STORAGE_KEY = "aurtrack.locale";

function readStoredLocale(): Locale {
  // 构建期覆盖，便于截图与语言回归测试。
  const forced = import.meta.env.VITE_AURTRACK_LOCALE;
  if (forced === "en" || forced === "zh-CN") return forced;
  if (typeof localStorage === "undefined") return "zh-CN";
  return localStorage.getItem(STORAGE_KEY) === "en" ? "en" : "zh-CN";
}

function applyDocumentLocale(value: Locale): void {
  if (typeof document !== "undefined") {
    document.documentElement.lang = value;
  }
}

let current = $state<Locale>(readStoredLocale());
applyDocumentLocale(current);

export function getLocale(): Locale {
  return current;
}

export function setLocale(next: Locale): void {
  current = next;
  applyDocumentLocale(next);
  if (typeof localStorage !== "undefined") {
    localStorage.setItem(STORAGE_KEY, next);
  }
}

export function toggleLocale(): void {
  setLocale(current === "zh-CN" ? "en" : "zh-CN");
}

export function t(
  key: MessageKey,
  params: Record<string, string | number> = {},
): string {
  const message = dictionaries[current][key] ?? zhCN[key];
  return typeof message === "function" ? message(params) : message;
}

export function formatNumber(value: number, digits = 0): string {
  return new Intl.NumberFormat(current, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  }).format(value);
}

export function formatClock(value: Date): string {
  return new Intl.DateTimeFormat(current, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(value);
}

function isMessageKey(key: string): key is MessageKey {
  return key in zhCN;
}

export function errorText(code: string, fallback: string): string {
  if (current === "zh-CN") return fallback;
  const key = `error.${code}`;
  return isMessageKey(key) ? t(key) : fallback;
}
