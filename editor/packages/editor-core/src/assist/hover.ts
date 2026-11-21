// Hover help: what the construct under the pointer is.
import type { Extension } from "@codemirror/state";
import { hoverTooltip } from "@codemirror/view";
import { parsed, parsedField } from "../state.js";
import type { Messages } from "./messages/types.js";

/** The help for the token `kind` written as `text`. */
export function describeToken(messages: Messages, kind: string, text: string): string | null {
  const general = messages.tokens[kind];
  if (!general) return null;
  if (kind === "block-attribute-list") {
    // Explain the style and the options the list names.
    const fields = text.replace(/^\[|\]\s*$/g, "").split(",").map((field) => field.trim());
    const style = /^[^#.%=]*/.exec(fields[0] ?? "")?.[0] ?? "";
    const notes = [];
    if (messages.styles[style]) notes.push(`${style} — ${messages.styles[style]}`);
    for (const option of text.match(/%[\w-]+/g) ?? []) {
      if (messages.styles[option]) notes.push(`${option} — ${messages.styles[option]}`);
    }
    return [...notes, general].join("\n");
  }
  if (kind === "attribute-entry") {
    const name = /^:!?([\w-]+)!?:/.exec(text)?.[1];
    const known = name ? messages.attributes[name] : undefined;
    return known ? `${name} — ${known}\n${general}` : general;
  }
  return general;
}

export function hoverHelp(messages: Messages): Extension {
  return [
    parsedField,
    hoverTooltip((view, pos) => {
      // The narrowest token at the pointer.
      let found: { from: number; to: number; kind: string } | null = null;
      parsed(view.state).tokens.between(pos, pos, (from, to, value) => {
        const name = (value.spec as { class?: string }).class;
        if (!name?.startsWith("adoc-")) return;
        if (!found || to - from < found.to - found.from) found = { from, to, kind: name.slice(5) };
      });
      const token = found as { from: number; to: number; kind: string } | null;
      if (!token) return null;
      const text = describeToken(messages, token.kind, view.state.sliceDoc(token.from, token.to));
      if (!text) return null;
      return {
        pos: token.from,
        end: token.to,
        above: true,
        create() {
          const dom = document.createElement("div");
          dom.className = "adoc-hover";
          for (const line of text.split("\n")) {
            const row = document.createElement("div");
            row.textContent = line;
            dom.append(row);
          }
          return { dom };
        },
      };
    }),
  ];
}
