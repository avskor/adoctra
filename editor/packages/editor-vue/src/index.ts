// Vue 3 bindings: a thin layer over `@adoctra/editor-core`. Written with
// render functions so the package builds with `tsc` alone.
import {
  createAdoctraEditor,
  mountPreview,
  type AdoctraEditor as Editor,
  type AdoctraEditorOptions,
  type AdoctraSnapshot,
  type Engine,
} from "@adoctra/editor-core";
import {
  defineComponent,
  h,
  onBeforeUnmount,
  ref,
  shallowRef,
  toValue,
  watch,
  type MaybeRefOrGetter,
  type PropType,
  type Ref,
  type ShallowRef,
} from "vue";

export type { Editor as AdoctraEditorHandle, AdoctraSnapshot, Engine };

type Options = Omit<AdoctraEditorOptions, "parent" | "engine">;

const NONE: AdoctraSnapshot = {
  revision: 0,
  parsedRevision: -1,
  view: null,
  outline: [],
  diagnostics: [],
  path: [],
  html: null,
};

/**
 * Mount an editor in the element bound to `element` once `engine` is there.
 * `snapshot` follows the editor (outline, diagnostics, block path, HTML).
 */
export function useAdoctraEditor(
  engine: MaybeRefOrGetter<Engine | null | undefined>,
  options: Options = {},
): {
  element: Ref<HTMLElement | null>;
  editor: ShallowRef<Editor | null>;
  snapshot: ShallowRef<AdoctraSnapshot>;
} {
  const element = ref<HTMLElement | null>(null);
  const editor = shallowRef<Editor | null>(null);
  const snapshot = shallowRef<AdoctraSnapshot>(NONE);
  const destroy = () => {
    editor.value?.destroy();
    editor.value = null;
    snapshot.value = NONE;
  };
  watch(
    [() => toValue(engine), element],
    ([engine, parent]) => {
      destroy();
      if (!engine || !parent) return;
      const created = createAdoctraEditor({ ...options, parent, engine });
      created.subscribe(() => (snapshot.value = created.getSnapshot()));
      snapshot.value = created.getSnapshot();
      editor.value = created;
    },
    { immediate: true, flush: "post" },
  );
  onBeforeUnmount(destroy);
  return { element, editor, snapshot };
}

/** `<AdoctraEditor v-model="text" :engine="engine" @ready="editor = $event" />` */
export const AdoctraEditor = defineComponent({
  name: "AdoctraEditor",
  props: {
    engine: { type: Object as PropType<Engine | null>, default: null },
    modelValue: { type: String, default: "" },
    name: { type: String, default: undefined },
    files: { type: Object as PropType<Record<string, string>>, default: undefined },
    readOnly: { type: Boolean, default: false },
    gutter: { type: Boolean, default: true },
    breadcrumbs: { type: Boolean, default: true },
  },
  emits: {
    "update:modelValue": (_value: string) => true,
    ready: (_editor: Editor | null) => true,
    snapshot: (_snapshot: AdoctraSnapshot) => true,
  },
  setup(props, { emit }) {
    const { element, editor, snapshot } = useAdoctraEditor(() => props.engine, {
      value: props.modelValue,
      name: props.name,
      files: props.files,
      readOnly: props.readOnly,
      gutter: props.gutter,
      breadcrumbs: props.breadcrumbs,
      onChange: (value) => emit("update:modelValue", value),
    });
    watch(editor, (value) => emit("ready", value));
    watch(snapshot, (value) => emit("snapshot", value));
    watch(
      () => props.modelValue,
      (value) => editor.value?.setValue(value),
    );
    return () => h("div", { ref: element });
  },
});

export const AdoctraPreview = defineComponent({
  name: "AdoctraPreview",
  props: {
    editor: { type: Object as PropType<Editor | null>, default: null },
    css: { type: String, default: undefined },
    syncScroll: { type: Boolean, default: true },
  },
  setup(props) {
    const element = ref<HTMLElement | null>(null);
    let preview: ReturnType<typeof mountPreview> | null = null;
    const destroy = () => {
      preview?.destroy();
      preview = null;
    };
    watch(
      [() => props.editor, element],
      ([editor, parent]) => {
        destroy();
        if (editor && parent) {
          preview = mountPreview(parent, editor, { css: props.css, syncScroll: props.syncScroll });
        }
      },
      { immediate: true, flush: "post" },
    );
    onBeforeUnmount(destroy);
    return () => h("div", { ref: element });
  },
});

/** The sections; the default slot replaces an entry (`{ entry, reveal }`). */
export const AdoctraOutline = defineComponent({
  name: "AdoctraOutline",
  props: { editor: { type: Object as PropType<Editor | null>, default: null } },
  setup(props, { slots }) {
    const snapshot = shallowRef<AdoctraSnapshot>(NONE);
    let unsubscribe = () => {};
    watch(
      () => props.editor,
      (editor) => {
        unsubscribe();
        snapshot.value = editor?.getSnapshot() ?? NONE;
        unsubscribe = editor?.subscribe(() => (snapshot.value = editor.getSnapshot())) ?? (() => {});
      },
      { immediate: true },
    );
    onBeforeUnmount(() => unsubscribe());
    return () =>
      h(
        "nav",
        { class: "adoc-outline" },
        snapshot.value.outline.map((entry) => {
          const reveal = () => props.editor?.revealOffset(entry.start);
          return h(
            "div",
            { key: `${entry.start}:${entry.title}`, "data-level": entry.level },
            slots.default?.({ entry, reveal }) ??
              h(
                "button",
                { type: "button", onClick: reveal, style: { paddingLeft: `${entry.level}em` } },
                entry.title,
              ),
          );
        }),
      );
  },
});
