// The command registry: every way of creating or changing an element is one
// command, shown by the toolbar, the slash menu and the block menu alike.
import type { EditorState, Transaction } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { blockPath } from "../blocks.js";
import { parsed } from "../state.js";
import type { Block } from "../types.js";

/** What a command changes: an `EditorView`, or a state with a dispatcher. */
export interface CommandTarget {
  readonly state: EditorState;
  dispatch(transaction: Transaction): void;
}

export type CommandGroup =
  | "inline"
  | "heading"
  | "list"
  | "block"
  | "insert"
  /** Acts on an existing block; offered by the block menu only. */
  | "blockAction";

/** What is at the cursor, for a command's pressed state. */
export interface AssistContext {
  state: EditorState;
  pos: number;
  /** The blocks around the cursor, outermost first. */
  path: readonly Block[];
  /** The kinds of the highlight tokens at the cursor (`strong`, `link`, …). */
  tokens: ReadonlySet<string>;
  /** The text of the cursor's line. */
  line: string;
}

export interface AssistCommand {
  id: string;
  group: CommandGroup;
  /** Search words beyond the label: the syntax's own names. */
  keywords: readonly string[];
  /** A short glyph for a button. */
  icon: string;
  /** The markup the command writes, as a sample. */
  example?: string;
  /**
   * Apply the command. A block action gets its block; `view` is there when
   * the command runs in an editor (focus, completion, folding).
   */
  run(target: CommandTarget, block?: Block, view?: EditorView): boolean;
  /** Whether the cursor is in what the command makes. */
  isActive?(context: AssistContext): boolean;
  /** Which blocks a block action is offered for. */
  appliesTo?(block: Block): boolean;
}

export function contextAt(state: EditorState): AssistContext {
  const pos = state.selection.main.head;
  const { blocks, tokens } = parsed(state);
  const kinds = new Set<string>();
  tokens.between(pos, pos, (_from, _to, value) => {
    const name = (value.spec as { class?: string }).class;
    if (name?.startsWith("adoc-")) kinds.add(name.slice(5));
  });
  return { state, pos, path: blockPath(blocks, pos), tokens: kinds, line: state.doc.lineAt(pos).text };
}

/** The ids of the commands whose element the cursor is in. */
export function activeCommands(commands: readonly AssistCommand[], state: EditorState): string[] {
  const context = contextAt(state);
  return commands.filter((command) => command.isActive?.(context)).map((command) => command.id);
}
