// Completion from the parse: cross-reference targets after `<<` and
// `xref:`, attribute names after `{`.
import { autocompletion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import type { Extension } from "@codemirror/state";
import { parsed, parsedField } from "../state.js";

function complete(context: CompletionContext): CompletionResult | null {
  const view = parsed(context.state).view;
  if (!view) return null;
  const xref = context.matchBefore(/(?:<<|xref:)[\w.:-]*$/);
  if (xref) {
    const from = xref.from + (xref.text.startsWith("<<") ? 2 : 5);
    const titles = new Map(view.outline.map((entry) => [entry.id, entry.title]));
    return {
      from,
      options: view.anchors.map(({ id }) => ({
        label: id,
        type: "constant",
        detail: titles.get(id) ?? undefined,
      })),
      validFor: /^[\w.:-]*$/,
    };
  }
  const attribute = context.matchBefore(/\{[\w-]*$/);
  if (attribute) {
    return {
      from: attribute.from + 1,
      options: Object.entries(view.attributes).map(([name, value]) => ({
        label: name,
        type: "variable",
        detail: value.length > 40 ? `${value.slice(0, 40)}…` : value,
        apply: `${name}}`,
      })),
      validFor: /^[\w-]*$/,
    };
  }
  return null;
}

export function completion(): Extension {
  return [parsedField, autocompletion({ override: [complete] })];
}
