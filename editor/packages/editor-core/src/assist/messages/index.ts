import { en } from "./en.js";
import { ru } from "./ru.js";
import type { Messages } from "./types.js";

export { en, ru };
export type { Messages };

export type Locale = "en" | "ru" | Messages;

export function resolveMessages(locale: Locale | undefined): Messages {
  if (locale === "ru") return ru;
  if (locale === undefined || locale === "en") return en;
  return locale;
}
