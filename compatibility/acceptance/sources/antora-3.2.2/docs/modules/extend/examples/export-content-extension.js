'use strict'

const { parse: parseHTML } = require('node-html-parser')

/**
 * An Antora extension that exports the content of publishable pages in plain text to a JSON
 * file along with the page URL and title.
 */
module.exports.register = function () {
  this.once('navigationBuilt', ({ playbook, contentCatalog, siteCatalog }) => {
    const siteUrl = playbook.site.url
    const component = 'dfcs'
    const version = ''
    const componentVersion = contentCatalog.getComponentVersion(component, version)
    const dfcsNavEntriesByUrl = getNavEntriesByUrl(componentVersion.navigation)
    const pages = contentCatalog
      .getPages((it) => it.src.component === component && it.src.version === version && it.pub)
      .map((page) => {
        const siteRelativeUrl = page.pub.url
        const articleDom = parseHTML(`<article>${page.contents}</article>`)
        // TODO might want to apply the sentence newline replacement per paragraph
        const text = articleDom.textContent
          .trim()
          .replace(/\n(\s*\n)+/g, '\n\n')
          .replace(/\.\n(?!\n)/g, '. ')
        const path = [componentVersion.title]
        path.push(...(dfcsNavEntriesByUrl[siteRelativeUrl]?.path?.map((it) => it.content) || []))
        return { url: siteUrl + siteRelativeUrl, title: page.title, text, path }
      })
    siteCatalog.addFile({
      contents: Buffer.from(JSON.stringify({ pages }, null, '  ')),
      out: { path: 'site-content.json' },
    })
  })
}

function getNavEntriesByUrl (items = [], accum = {}, path = []) {
  items.forEach((item) => {
    if (item.urlType === 'internal') accum[item.url.split('#')[0]] = { item, path: path.concat(item) }
    getNavEntriesByUrl(item.items, accum, item.content ? path.concat(item) : path)
  })
  return accum
}
