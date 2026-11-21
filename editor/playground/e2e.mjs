// End-to-end check of the playground in a real browser:
//   npm run build:wasm && npm run build && npm run e2e
// Uses playwright-core with an installed Chrome (CHROME=/path overrides).
import { chromium } from "playwright-core";
import { createServer } from "vite";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";

const root = fileURLToPath(new URL(".", import.meta.url));
const server = await createServer({ root, logLevel: "error", server: { port: 0 } });
await server.listen();
const base = server.resolvedUrls.local[0];
const browser = await chromium.launch({
  executablePath: process.env.CHROME ?? "/usr/bin/google-chrome",
});
const shots = process.env.SHOTS;
let failed = false;

async function check(name, path) {
  const page = await browser.newPage({ viewport: { width: 1400, height: 800 } });
  const errors = [];
  page.on("pageerror", (error) => errors.push(String(error)));
  page.on("console", (message) => message.type() === "error" && errors.push(message.text()));
  try {
    await page.goto(base + path);
    // Highlighting from the parser's tokens.
    await page.waitForSelector(".cm-content .adoc-heading", { timeout: 30000 });
    assert.ok(await page.locator(".cm-content .adoc-strong").count(), "strong token");
    assert.ok(await page.locator(".cm-content .adoc-listing-delimiter").count(), "fence token");
    // The block gutter names block types.
    const labels = await page.locator(".adoc-block-label").allTextContents();
    for (const label of ["H1", "H2", "¶", "sh", "NOTE", "•", "••", "table", "quote"]) {
      assert.ok(labels.includes(label), `gutter label ${label} in ${labels}`);
    }
    // The outline lists the title and the sections.
    const outline = await page.locator(".adoc-outline button").allTextContents();
    assert.deepEqual(outline, ["Руководство по adoctra", "Установка", "Возможности"]);
    // The preview shows the converted document with source positions.
    const frame = page.frameLocator("iframe.adoc-preview");
    await frame.locator("h2#_установка").waitFor();
    assert.ok(await frame.locator("[data-source-start]").count() > 5, "marked elements");
    assert.equal(await frame.locator("table td").first().textContent(), "HTML5");

    // Moving the cursor into the listing changes the active block.
    await page.locator(".cm-content .adoc-listing-text").first().click();
    await page.waitForFunction(
      () => document.querySelector(".adoc-breadcrumbs")?.textContent.includes("listing (sh)"),
    );
    assert.ok(await page.locator(".adoc-gutter-active").count() >= 5, "active bar spans the block");
    assert.equal(await frame.locator(".adoc-preview-active").count(), 1);

    // ---- authoring aids ------------------------------------------------
    const content = page.locator(".cm-content");
    const tableCount = (count) =>
      page.waitForFunction(
        (expected) =>
          document.querySelector("iframe.adoc-preview").contentDocument.querySelectorAll("table").length === expected,
        count,
      );
    // The toolbar shows where the cursor is.
    await page.locator(".cm-content .adoc-strong").first().click();
    await page.waitForSelector('.adoc-tool[data-command="strong"][aria-pressed="true"]');
    // The slash menu inserts a table found by its Russian name.
    await page.keyboard.press("Control+End");
    await page.keyboard.type("\n\n/таб");
    await page.locator(".cm-tooltip-autocomplete li", { hasText: "Таблица" }).first().waitFor();
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => document.querySelector(".cm-content").textContent.includes("Header 1"));
    assert.ok(!(await content.textContent()).includes("/таб"), "the query is removed");
    await tableCount(2);
    // The toolbar's table picker inserts a table of the chosen size after it.
    await page.locator('.adoc-tool[data-command="table"]').click();
    await page.locator('.adoc-table-cell[data-rows="3"][data-columns="3"]').click();
    await tableCount(3);
    assert.equal(await frame.locator("table").nth(2).locator("tbody tr").count(), 3);
    await page.keyboard.press("Escape");
    // A toolbar menu lists commands by their labels.
    await page.locator('.adoc-tool[data-command="menu:admonitions"]').click();
    assert.deepEqual(
      (await page.locator(".adoc-menu-admonitions .adoc-menu-item").allTextContents()).map((label) =>
        label.replace(/^[^А-Яа-я]+/, ""),
      ),
      ["Примечание", "Совет", "Важно", "Предупреждение", "Осторожно"],
    );
    await page.keyboard.press("Escape");
    assert.equal(await page.locator(".adoc-menu").count(), 0);
    // The block menu turns the first paragraph into a note.
    await page.locator(".cm-content").click();
    await page.keyboard.press("Control+Home");
    await page.waitForFunction(() => document.querySelector(".cm-scroller").scrollTop === 0);
    await page.locator(".adoc-block-label", { hasText: "¶" }).first().click();
    await page.locator(".adoc-menu-block .adoc-menu-item", { hasText: "Сделать примечанием" }).click();
    await page.waitForFunction(() =>
      document.querySelector(".cm-content").textContent.includes("NOTE: Вводный абзац"),
    );
    // Hovering an attribute list explains its style.
    await page.locator(".cm-content .adoc-block-attribute-list").first().hover();
    await page.locator(".adoc-hover", { hasText: "source —" }).waitFor();
    await page.mouse.move(5, 5);
    if (shots) await page.screenshot({ path: `${shots}/${name}-assist.png` });

    // List levels: Enter continues the list, Tab nests the item, Shift+Tab
    // lifts it; the toolbar buttons do the same.
    const nestedItems = (count) =>
      page.waitForFunction(
        (expected) =>
          [...document.querySelector("iframe.adoc-preview").contentDocument.querySelectorAll("ul ul li")].filter(
            (item) => item.textContent.trim() === "вложенный",
          ).length === expected,
        count,
      );
    await page.locator(".cm-content").click();
    await page.keyboard.press("Control+End");
    await page.keyboard.type("\n\n* первый");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Tab");
    await page.keyboard.type("вложенный");
    await page.waitForFunction(() => document.querySelector(".cm-content").textContent.includes("** вложенный"));
    await nestedItems(1);
    await page.keyboard.press("Shift+Tab");
    await nestedItems(0);
    await page.locator('.adoc-tool[data-command="list-indent"]').click();
    await nestedItems(1);
    await page.locator('.adoc-tool[data-command="list-outdent"]').click();
    await nestedItems(0);
    await page.keyboard.press("Tab");
    await nestedItems(1);
    // Enter twice on empty items leaves the list.
    await page.keyboard.press("Enter");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Enter");
    await page.keyboard.type("после списка");
    await page.waitForFunction(() =>
      [...document.querySelector("iframe.adoc-preview").contentDocument.querySelectorAll("p")].some(
        (p) => p.textContent === "после списка" && !p.closest("li"),
      ),
    );

    // Each aid switches off on its own.
    await page.locator("#toggle-toolbar").uncheck();
    await page.waitForFunction(() => !document.querySelector(".adoc-toolbar"));
    assert.ok(await page.locator(".adoc-breadcrumbs").count(), "the rest stays");
    await page.locator("#toggle-hover").uncheck();
    await page.locator(".cm-content .adoc-block-attribute-list").first().hover();
    await page.waitForTimeout(700);
    assert.equal(await page.locator(".adoc-hover").count(), 0, "no hover help");
    await page.locator("#toggle-blockMenu").uncheck();
    await page.locator(".adoc-block-label", { hasText: "sh" }).first().click();
    await page.waitForTimeout(200);
    assert.equal(await page.locator(".adoc-menu").count(), 0, "no block menu");
    await page.locator("#toggle-slashMenu").uncheck();
    await page.locator(".cm-content").click();
    await page.keyboard.press("Control+End");
    await page.keyboard.type("\n\n/");
    await page.waitForTimeout(500);
    assert.equal(await page.locator(".cm-tooltip-autocomplete").count(), 0, "no slash menu");
    await page.keyboard.press("Backspace");
    // Without the list keys, Tab indents the line as in any editor.
    await page.evaluate(() => document.querySelector("#toggle-listKeys").click());
    await page.locator(".cm-content").click();
    await page.keyboard.press("Control+End");
    await page.keyboard.type("\n\n* пункт");
    await page.keyboard.press("Tab");
    await page.waitForTimeout(200);
    assert.ok(!(await content.textContent()).includes("** пункт"), "Tab does not change the level");
    await page.evaluate(() => document.querySelector("#toggle-listKeys").click());
    await page.keyboard.press("Tab");
    await page.waitForFunction(() => document.querySelector(".cm-content").textContent.includes("** пункт"));
    // …and on again.
    await page.locator("#toggle-toolbar").check();
    await page.waitForSelector(".adoc-toolbar");
    await page.locator("#toggle-slashMenu").check();
    await page.locator(".cm-content").click();
    await page.keyboard.press("Control+End");
    // Leave the list first: the slash has to be alone on its line.
    await page.keyboard.press("Enter");
    await page.keyboard.press("Enter");
    await page.keyboard.press("Enter");
    await page.keyboard.type("/");
    await page.locator(".cm-tooltip-autocomplete li", { hasText: "Жирный" }).waitFor();
    await page.keyboard.press("Escape");
    await page.keyboard.press("Backspace");

    // Typing reparses: a new section reaches outline and preview; an
    // unterminated block is diagnosed.
    await page.keyboard.press("Control+End");
    await page.keyboard.type("\n\n== Новый раздел 😀\n\n----\nне закрыт");
    await page.waitForFunction(
      () => [...document.querySelectorAll(".adoc-outline button")].some((b) => b.textContent === "Новый раздел 😀"),
    );
    await frame.locator("h2", { hasText: "Новый раздел" }).waitFor();
    await page.waitForSelector(".cm-lintRange-error");
    // Clicking the outline moves the cursor there.
    await page.locator(".adoc-outline button", { hasText: "Установка" }).click();
    await page.waitForFunction(
      () => document.querySelector(".adoc-breadcrumbs")?.textContent.includes("section H2"),
    );
    // Clicking the preview moves the cursor to the block's source.
    await frame.locator("table").first().click();
    await page.waitForFunction(
      () => document.querySelector(".adoc-breadcrumbs")?.textContent.includes("table"),
    );
    assert.deepEqual(errors, []);
    if (shots) await page.screenshot({ path: `${shots}/${name}.png` });
    console.log(`ok ${name}`);
  } catch (error) {
    failed = true;
    console.error(`FAILED ${name}:`, error.message, errors);
    if (shots) await page.screenshot({ path: `${shots}/${name}-failed.png` });
  }
  await page.close();
}

await check("core", "");
await check("react", "react.html");
await check("vue", "vue.html");
await browser.close();
await server.close();
process.exit(failed ? 1 : 0);
