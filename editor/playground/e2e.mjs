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
    for (const label of ["H1", "H2", "¶", "sh", "NOTE", "ul", "table", "quote"]) {
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
    await frame.locator("table").click();
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
