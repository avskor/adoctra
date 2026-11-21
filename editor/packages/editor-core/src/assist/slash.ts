// The slash menu: `/` on a line of its own lists the commands.
import type { Completion, CompletionContext, CompletionResult, CompletionSource } from "@codemirror/autocomplete";
import type { EditorView } from "@codemirror/view";
import { blockAt } from "../blocks.js";
import { parsed } from "../state.js";
import type { AssistCommand } from "./commands.js";
import { en } from "./messages/en.js";
import type { Messages } from "./messages/types.js";

const VERBATIM = new Set(["listing", "literal", "pass", "stem"]);

/** The query after a `/` that is alone on its line, or `null`. */
export function slashQuery(lineText: string, column: number): { from: number; query: string } | null {
  const match = /^([ \t]*)\/([^\s/]*)$/.exec(lineText.slice(0, column));
  if (!match || lineText.slice(column).trim() !== "") return null;
  return { from: match[1]!.length, query: match[2]! };
}

/** The commands matching `query` by label (in `messages` and English), id or keyword. */
export function searchCommands(
  commands: readonly AssistCommand[],
  messages: Messages,
  query: string,
): AssistCommand[] {
  const needle = query.toLowerCase();
  const rank = (command: AssistCommand): number => {
    const names = [messages.commands[command.id], en.commands[command.id], command.id]
      .filter((name): name is string => name !== undefined)
      .map((name) => name.toLowerCase());
    if (names.some((name) => name.startsWith(needle))) return 0;
    if (names.some((name) => name.split(/[\s(-]+/).some((word) => word.startsWith(needle)))) return 1;
    if (command.keywords.some((word) => word.toLowerCase().startsWith(needle))) return 2;
    if (names.some((name) => name.includes(needle))) return 3;
    if (command.keywords.some((word) => word.toLowerCase().includes(needle))) return 4;
    return -1;
  };
  return commands
    .filter((command) => command.group !== "blockAction")
    .map((command, index) => ({ command, index, rank: rank(command) }))
    .filter((entry) => entry.rank >= 0)
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map((entry) => entry.command);
}

export function slashSource(commands: readonly AssistCommand[], messages: Messages): CompletionSource {
  return (context: CompletionContext): CompletionResult | null => {
    const line = context.state.doc.lineAt(context.pos);
    const found = slashQuery(line.text, context.pos - line.from);
    if (!found) return null;
    const around = blockAt(parsed(context.state).blocks, context.pos);
    if (around && VERBATIM.has(around.kind)) return null;
    const from = line.from + found.from;
    const options: Completion[] = searchCommands(commands, messages, found.query).map(
      (command, index) => ({
        label: `/${messages.commands[command.id] ?? command.id}`,
        displayLabel: messages.commands[command.id] ?? command.id,
        detail: messages.groups[command.group],
        type: "adoc-command",
        boost: -index,
        ...(command.example
          ? {
              info: () => {
                const sample = document.createElement("pre");
                sample.className = "adoc-slash-example";
                sample.textContent = command.example!;
                return sample;
              },
            }
          : {}),
        apply: (view: EditorView, _completion: Completion, start: number, end: number) => {
          view.dispatch({ changes: { from: start, to: end }, selection: { anchor: start } });
          command.run(view, undefined, view);
          view.focus();
        },
      }),
    );
    return { from, options, filter: false };
  };
}
