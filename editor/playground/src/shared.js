import { createWorkerEngine } from "@adoctra/editor-core";

export const engine = () =>
  createWorkerEngine(new Worker(new URL("./adoctra.worker.js", import.meta.url), { type: "module" }));

export const sample = `= Руководство по adoctra
:toc:
:product: adoctra

Вводный абзац с *жирным*, _курсивом_ и \`кодом\`; продукт — {product}.

== Установка

[[install]]
.Сборка из исходников
[source,sh]
----
cargo install adoctra-cli
----

NOTE: Нужен Rust 1.94 или новее.

== Возможности

* Разбор, устойчивый к ошибкам
** диагностики вместо отказов
* Подсветка из того же разбора

.Бэкенды
|===
|Формат |Крейт

|HTML5 |adoctra-html
|DOCX |adoctra-docx
|===

См. <<install>> и <<_возможности>>.

[quote, Автор]
____
Цитата в блоке.
____
`;
