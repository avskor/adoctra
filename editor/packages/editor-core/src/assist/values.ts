// Completion of values the syntax expects: block styles, source languages,
// document attributes, include targets.
import type { CompletionContext, CompletionResult, CompletionSource } from "@codemirror/autocomplete";
import type { Messages } from "./messages/types.js";

/** Languages offered after `[source,`; any other name can still be typed. */
export const LANGUAGES = [
  "asciidoc", "bash", "c", "clojure", "cpp", "csharp", "css", "diff", "dockerfile", "elixir", "erlang", "go",
  "graphql", "groovy", "haskell", "html", "ini", "java", "javascript", "json", "kotlin", "lua", "makefile",
  "markdown", "nginx", "objective_c", "perl", "php", "powershell", "properties", "protobuf", "python", "r",
  "ruby", "rust", "scala", "scss", "sh", "shell", "sql", "swift", "terraform", "toml", "typescript", "vue",
  "xml", "yaml",
];

export function valueSource(messages: Messages, files: () => readonly string[] = () => []): CompletionSource {
  return (context: CompletionContext): CompletionResult | null => {
    const line = context.state.doc.lineAt(context.pos);
    const before = line.text.slice(0, context.pos - line.from);

    const language = /^\[source,\s*([\w+#-]*)$/.exec(before);
    if (language) {
      return {
        from: context.pos - language[1]!.length,
        options: LANGUAGES.map((label) => ({ label, type: "keyword" })),
        validFor: /^[\w+#-]*$/,
      };
    }
    // The style, or an option anywhere in the list.
    const style = /^\[(?:[^\]=]*,\s*)?([%\w"=-]*)$/.exec(before);
    if (style && !before.startsWith("[[")) {
      const first = !before.includes(",");
      return {
        from: context.pos - style[1]!.length,
        options: Object.entries(messages.styles)
          .filter(([name]) => first || name.startsWith("%") || name.includes("="))
          .map(([label, detail]) => ({ label, detail, type: label.startsWith("%") ? "property" : "keyword" })),
        validFor: /^[%\w"=,-]*$/,
      };
    }
    const attribute = /^:([\w-]*)$/.exec(before);
    if (attribute) {
      return {
        from: context.pos - attribute[1]!.length,
        options: Object.entries(messages.attributes).map(([label, detail]) => ({
          label,
          detail,
          type: "variable",
          apply: `${label}: `,
        })),
        validFor: /^[\w-]*$/,
      };
    }
    const include = /^include::([^\[\s]*)$/.exec(before);
    if (include) {
      const names = files();
      if (names.length === 0) return null;
      return {
        from: context.pos - include[1]!.length,
        options: names.map((label) => ({ label, type: "text", apply: `${label}[]` })),
        validFor: /^[^\[\s]*$/,
      };
    }
    return null;
  };
}
