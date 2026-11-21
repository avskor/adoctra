import type { CommandGroup } from "../commands.js";

/** The words of the assist surfaces, in one language. */
export interface Messages {
  /** Labels by command id. */
  commands: Record<string, string>;
  groups: Record<CommandGroup, string>;
  /** Labels of the toolbar's menus (`headings`, `more`, …) and the table picker. */
  menus: Record<string, string>;
  tableSize(rows: number, columns: number): string;
  /** What a highlighted construct is, by token kind. */
  tokens: Record<string, string>;
  /** Block styles and options (`[source]`, `[%collapsible]`, …). */
  styles: Record<string, string>;
  /** Document attributes (`:toc:`, …). */
  attributes: Record<string, string>;
}
