export type MessageParams = Record<string, string | number>;

export type Message = string | ((params: MessageParams) => string);
